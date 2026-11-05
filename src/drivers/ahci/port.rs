use x86_64::structures::paging::PhysFrame;

use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::hhdm_offset;

use super::hba::{ATA_DEV_BUSY, ATA_DEV_DRQ, ATA_DEV_ERR};

/// Блочный накопитель Serial ATA (SATA SSD / HDD) под управлением контроллера AHCI
pub struct AhciDisk {
    pub(crate) port_base: u64,
    pub(crate) sector_count: u64,
    pub(crate) dma_frame: PhysFrame,
}

unsafe impl Send for AhciDisk {}
unsafe impl Sync for AhciDisk {}

impl AhciDisk {
    pub fn new(port_base: u64, dma_frame: PhysFrame) -> Self {
        Self {
            port_base,
            sector_count: 0,
            dma_frame,
        }
    }

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

    pub fn execute_cmd(&mut self, is_write: bool, ata_cmd: u8, lba: u64, count: u16) -> Result<(), BlockDeviceError> {
        self.wait_not_busy()?;

        let base_phys = self.dma_frame.start_address().as_u64();
        let base_virt = self.dma_virt();

        let cmd_tab_phys = base_phys + 0x500;
        let bounce_phys = base_phys + 0x800;

        let cmd_header_virt = base_virt as *mut u32;
        let cmd_tab_virt = unsafe { base_virt.add(0x500) };

        let byte_count = (count as u32) * 512;

        unsafe {
            let flags = (5u32) | ((is_write as u32) << 6) | (1u32 << 16);
            cmd_header_virt.write_volatile(flags);
            cmd_header_virt.add(1).write_volatile(0);
            cmd_header_virt.add(2).write_volatile(cmd_tab_phys as u32);
            cmd_header_virt.add(3).write_volatile((cmd_tab_phys >> 32) as u32);
            cmd_header_virt.add(4).write_volatile(0);
            cmd_header_virt.add(5).write_volatile(0);
            cmd_header_virt.add(6).write_volatile(0);
            cmd_header_virt.add(7).write_volatile(0);

            core::ptr::write_bytes(cmd_tab_virt, 0, 128);
            let cfis = cmd_tab_virt;
            *cfis.add(0) = 0x27; // FIS_TYPE_REG_H2D
            *cfis.add(1) = 0x80; // Command
            *cfis.add(2) = ata_cmd;
            *cfis.add(4) = (lba & 0xFF) as u8;
            *cfis.add(5) = ((lba >> 8) & 0xFF) as u8;
            *cfis.add(6) = ((lba >> 16) & 0xFF) as u8;
            *cfis.add(7) = 0x40; // LBA mode
            *cfis.add(8) = ((lba >> 24) & 0xFF) as u8;
            *cfis.add(9) = ((lba >> 32) & 0xFF) as u8;
            *cfis.add(10) = ((lba >> 40) & 0xFF) as u8;
            *cfis.add(12) = (count & 0xFF) as u8;
            *cfis.add(13) = ((count >> 8) & 0xFF) as u8;

            let prdt_ptr = cmd_tab_virt.add(0x80) as *mut u32;
            prdt_ptr.write_volatile(bounce_phys as u32);
            prdt_ptr.add(1).write_volatile((bounce_phys >> 32) as u32);
            prdt_ptr.add(2).write_volatile(0);
            prdt_ptr.add(3).write_volatile((byte_count - 1) | (1u32 << 31));

            let is_ptr = (self.port_base + 0x10) as *mut u32;
            is_ptr.write_volatile(0xFFFF_FFFF);

            let ci_ptr = (self.port_base + 0x38) as *mut u32;
            ci_ptr.write_volatile(1);

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
