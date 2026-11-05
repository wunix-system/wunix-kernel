use alloc::sync::Arc;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::{allocate_frame, hhdm_offset, map_kernel_mmio};
use crate::serial_println;

const HBA_GHC_AE: u32 = 1 << 31;

const PORT_CMD_ST: u32 = 1 << 0;
const PORT_CMD_FRE: u32 = 1 << 4;
const PORT_CMD_FR: u32 = 1 << 14;
const PORT_CMD_CR: u32 = 1 << 15;

const ATA_DEV_BUSY: u32 = 1 << 7;
const ATA_DEV_DRQ: u32 = 1 << 3;
const ATA_DEV_ERR: u32 = 1 << 0;

const SATA_SIG_ATA: u32 = 0x0000_0101;

/// Блочный диск SATA под управлением контроллера AHCI 1.0
pub struct AhciDisk {
    port_base: u64,
    sector_count: u64,
    dma_frame: PhysFrame,
}

unsafe impl Send for AhciDisk {}
unsafe impl Sync for AhciDisk {}

impl AhciDisk {
    fn dma_virt(&self) -> *mut u8 {
        (hhdm_offset() + self.dma_frame.start_address().as_u64()) as *mut u8
    }

    fn wait_not_busy(&self) -> Result<(), BlockDeviceError> {
        let tfd_ptr = (self.port_base + 0x20) as *mut u32;
        for _ in 0..100_000 {
            let tfd = unsafe { tfd_ptr.read_volatile() };
            if (tfd & (ATA_DEV_BUSY | ATA_DEV_DRQ)) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(BlockDeviceError::IoError)
    }

    fn execute_cmd(&mut self, is_write: bool, ata_cmd: u8, lba: u64, count: u16) -> Result<(), BlockDeviceError> {
        self.wait_not_busy()?;

        let base_phys = self.dma_frame.start_address().as_u64();
        let base_virt = self.dma_virt();

        let _cmd_list_phys = base_phys;
        let cmd_tab_phys = base_phys + 0x500;
        let bounce_phys = base_phys + 0x800;

        let cmd_header_virt = base_virt as *mut u32;
        let cmd_tab_virt = unsafe { base_virt.add(0x500) };

        let byte_count = (count as u32) * 512;

        unsafe {
            // Command Header 0: cfl = 5 (20 байт FIS), w = is_write, prdtl = 1
            let flags = (5u32) | ((is_write as u32) << 6) | (1u32 << 16);
            cmd_header_virt.write_volatile(flags);
            cmd_header_virt.add(1).write_volatile(0); // prdbc
            cmd_header_virt.add(2).write_volatile(cmd_tab_phys as u32);
            cmd_header_virt.add(3).write_volatile((cmd_tab_phys >> 32) as u32);
            cmd_header_virt.add(4).write_volatile(0);
            cmd_header_virt.add(5).write_volatile(0);
            cmd_header_virt.add(6).write_volatile(0);
            cmd_header_virt.add(7).write_volatile(0);

            // Command Table: Command FIS (Reg H2D 0x27)
            core::ptr::write_bytes(cmd_tab_virt, 0, 128);
            let cfis = cmd_tab_virt;
            *cfis.add(0) = 0x27; // FIS_TYPE_REG_H2D
            *cfis.add(1) = 0x80; // Command bit
            *cfis.add(2) = ata_cmd;
            *cfis.add(3) = 0;

            *cfis.add(4) = (lba & 0xFF) as u8;
            *cfis.add(5) = ((lba >> 8) & 0xFF) as u8;
            *cfis.add(6) = ((lba >> 16) & 0xFF) as u8;
            *cfis.add(7) = 0x40; // LBA mode

            *cfis.add(8) = ((lba >> 24) & 0xFF) as u8;
            *cfis.add(9) = ((lba >> 32) & 0xFF) as u8;
            *cfis.add(10) = ((lba >> 40) & 0xFF) as u8;
            *cfis.add(11) = 0;

            *cfis.add(12) = (count & 0xFF) as u8;
            *cfis.add(13) = ((count >> 8) & 0xFF) as u8;
            *cfis.add(14) = 0;
            *cfis.add(15) = 0;

            // PRDT 0: offset 0x80 внутри Command Table
            let prdt_ptr = cmd_tab_virt.add(0x80) as *mut u32;
            prdt_ptr.write_volatile(bounce_phys as u32);
            prdt_ptr.add(1).write_volatile((bounce_phys >> 32) as u32);
            prdt_ptr.add(2).write_volatile(0);
            prdt_ptr.add(3).write_volatile((byte_count - 1) | (1u32 << 31));

            // Очищаем статус прерываний порта
            let is_ptr = (self.port_base + 0x10) as *mut u32;
            is_ptr.write_volatile(0xFFFF_FFFF);

            // Выдаем команду в слот 0 (PxCI bit 0)
            let ci_ptr = (self.port_base + 0x38) as *mut u32;
            ci_ptr.write_volatile(1);

            // Ожидаем завершения команды хост-контроллером
            let tfd_ptr = (self.port_base + 0x20) as *mut u32;
            for _ in 0..200_000 {
                if (ci_ptr.read_volatile() & 1) == 0 {
                    let tfd = tfd_ptr.read_volatile();
                    if (tfd & ATA_DEV_ERR) != 0 {
                        return Err(BlockDeviceError::IoError);
                    }
                    return Ok(());
                }
                core::hint::spin_loop();
            }
        }

        Err(BlockDeviceError::IoError)
    }
}

impl BlockDevice for AhciDisk {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        let bounce_virt = unsafe { self.dma_virt().add(0x800) };

        for sec in 0..count {
            let lba = start_sector + sec as u64;
            self.execute_cmd(false, 0x25, lba, 1)?; // READ DMA EXT

            unsafe {
                core::ptr::copy_nonoverlapping(
                    bounce_virt,
                    buf.as_mut_ptr().add(sec * 512),
                    512,
                );
            }
        }
        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        let bounce_virt = unsafe { self.dma_virt().add(0x800) };

        for sec in 0..count {
            let lba = start_sector + sec as u64;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buf.as_ptr().add(sec * 512),
                    bounce_virt,
                    512,
                );
            }
            self.execute_cmd(true, 0x35, lba, 1)?; // WRITE DMA EXT
        }
        Ok(())
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

/// Проверяет PCI устройство контроллера SATA/AHCI и регистрирует найденные накопители
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    // Class 01 (Mass Storage), Subclass 06 (SATA)
    if dev.class_code != 0x01 || (dev.subclass != 0x06 && dev.subclass != 0x04) {
        return false;
    }

    dev.enable_memory_space();
    dev.enable_bus_mastering();

    let abar_bar = match dev.bar(5) {
        Some(b) => b,
        None => return false,
    };

    let abar_virt = match map_kernel_mmio(abar_bar.address, abar_bar.size as usize) {
        Ok(v) => v.as_u64(),
        Err(e) => {
            serial_println!("ahci: failed to map ABAR MMIO: {}", e);
            return false;
        }
    };

    serial_println!("ahci: initializing AHCI 1.0 controller at {}", dev.address);

    unsafe {
        let ghc_ptr = (abar_virt + 0x04) as *mut u32;
        ghc_ptr.write_volatile(ghc_ptr.read_volatile() | HBA_GHC_AE);

        let pi = ((abar_virt + 0x0C) as *mut u32).read_volatile();
        for port_idx in 0..32 {
            if (pi & (1 << port_idx)) == 0 {
                continue;
            }

            let port_base = abar_virt + 0x100 + (port_idx as u64) * 0x80;
            let ssts = ((port_base + 0x28) as *mut u32).read_volatile();
            let det = ssts & 0x0F;
            let ipm = (ssts >> 8) & 0x0F;

            // DET == 3: устройство обнаружено и связь PHY установлена, IPM == 1: активное состояние
            if det != 3 || ipm != 1 {
                continue;
            }

            let sig = ((port_base + 0x24) as *mut u32).read_volatile();
            if sig != SATA_SIG_ATA {
                // Игнорируем оптические приводы SATAPI или SEMB
                continue;
            }

            // Выделяем DMA фрейм для структур команд порта
            let dma_frame = match allocate_frame() {
                Some(f) => f,
                None => {
                    serial_println!("ahci: failed to allocate DMA frame for port {}", port_idx);
                    continue;
                }
            };

            let base_phys = dma_frame.start_address().as_u64();
            let base_virt = (hhdm_offset() + base_phys) as *mut u8;
            core::ptr::write_bytes(base_virt, 0, 4096);

            // Останавливаем порт перед перенастройкой адресов
            let cmd_ptr = (port_base + 0x18) as *mut u32;
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() & !PORT_CMD_ST);
            while (cmd_ptr.read_volatile() & PORT_CMD_CR) != 0 {
                core::hint::spin_loop();
            }
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() & !PORT_CMD_FRE);
            while (cmd_ptr.read_volatile() & PORT_CMD_FR) != 0 {
                core::hint::spin_loop();
            }

            // Настраиваем адреса Command List (offset 0) и Received FIS (offset 0x400)
            let clb_ptr = port_base as *mut u32;
            clb_ptr.write_volatile(base_phys as u32);
            clb_ptr.add(1).write_volatile((base_phys >> 32) as u32);

            let fb_ptr = (port_base + 0x08) as *mut u32;
            fb_ptr.write_volatile((base_phys + 0x400) as u32);
            fb_ptr.add(1).write_volatile(((base_phys + 0x400) >> 32) as u32);

            // Очищаем ошибки и прерывания
            ((port_base + 0x30) as *mut u32).write_volatile(0xFFFF_FFFF);
            ((port_base + 0x10) as *mut u32).write_volatile(0xFFFF_FFFF);

            // Запускаем FRE и ST
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() | PORT_CMD_FRE);
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() | PORT_CMD_ST);

            let mut disk = AhciDisk {
                port_base,
                sector_count: 0,
                dma_frame,
            };

            // Выполняем IDENTIFY DEVICE (0xEC) для определения геометрии
            if disk.execute_cmd(false, 0xEC, 0, 1).is_ok() {
                let id_buf = (base_virt.add(0x800)) as *const u16;
                let lba28_sectors = (id_buf.add(60).read() as u32) | ((id_buf.add(61).read() as u32) << 16);
                let lba48_supp = (id_buf.add(83).read() & (1 << 10)) != 0;
                let sector_count = if lba48_supp {
                    let lba48 = (id_buf.add(100).read() as u64)
                        | ((id_buf.add(101).read() as u64) << 16)
                        | ((id_buf.add(102).read() as u64) << 32)
                        | ((id_buf.add(103).read() as u64) << 48);
                    if lba48 > 0 { lba48 } else { lba28_sectors as u64 }
                } else {
                    lba28_sectors as u64
                };

                disk.sector_count = sector_count;
                let letter = (b'a' + (*block_count as u8).min(25)) as char;
                let name = alloc::format!("sd{}", letter);
                let cap_mb = (sector_count * 512) / (1024 * 1024);

                serial_println!(
                    "ahci: port {} registered /dev/{} ({} MB, {} sectors)",
                    port_idx,
                    name,
                    cap_mb,
                    sector_count
                );

                dm.register_block_device(name, Arc::new(Mutex::new(disk)));
                *block_count += 1;
            } else {
                serial_println!("ahci: port {} IDENTIFY command failed", port_idx);
            }
        }
    }

    true
}
