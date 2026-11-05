pub mod fis;
pub mod hba;
pub mod port;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::memory::{allocate_frame, hhdm_offset, map_kernel_mmio};
use crate::serial_println;

pub use port::AhciDisk;
use hba::{HBA_GHC_AE, PORT_CMD_CR, PORT_CMD_FR, PORT_CMD_FRE, PORT_CMD_ST, SATA_SIG_ATA};

/// Проверяет PCI устройство контроллера SATA/AHCI и регистрирует найденные накопители
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    // Class 01 (Mass Storage), Subclass 06 (SATA)
    if dev.class_code != 0x01 || (dev.subclass != 0x06 && dev.subclass != 0x04) {
        return false;
    }

    dev.driver_name = Some("ahci");

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

            // DET == 3: связь PHY установлена, IPM == 1: активное состояние интерфейса
            if det != 3 || ipm != 1 {
                continue;
            }

            let sig = ((port_base + 0x24) as *mut u32).read_volatile();
            if sig != SATA_SIG_ATA {
                continue;
            }

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

            // Настраиваем адреса Command List и Received FIS
            let clb_ptr = port_base as *mut u32;
            clb_ptr.write_volatile(base_phys as u32);
            clb_ptr.add(1).write_volatile((base_phys >> 32) as u32);

            let fb_ptr = (port_base + 0x08) as *mut u32;
            fb_ptr.write_volatile((base_phys + 0x400) as u32);
            fb_ptr.add(1).write_volatile(((base_phys + 0x400) >> 32) as u32);

            // Сбрасываем флаги прерываний и ошибок
            ((port_base + 0x30) as *mut u32).write_volatile(0xFFFF_FFFF);
            ((port_base + 0x10) as *mut u32).write_volatile(0xFFFF_FFFF);

            // Запускаем FRE и ST
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() | PORT_CMD_FRE);
            cmd_ptr.write_volatile(cmd_ptr.read_volatile() | PORT_CMD_ST);

            let mut disk = AhciDisk::new(port_base, dma_frame);

            // Выполняем IDENTIFY DEVICE (0xEC)
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
                    port_idx, name, cap_mb, sector_count
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_ahci_struct_sizes() {
        assert_eq!(core::mem::size_of::<fis::FisRegH2D>(), 20);
        assert_eq!(core::mem::size_of::<hba::CommandHeader>(), 32);
        assert_eq!(core::mem::size_of::<hba::PrdtEntry>(), 16);
    }
}
