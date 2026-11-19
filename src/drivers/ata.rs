use x86_64::instructions::port::Port;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};

const ATA_DATA: u16 = 0x00;
const ATA_SECCOUNT: u16 = 0x02;
const ATA_LBA_LO: u16 = 0x03;
const ATA_LBA_MID: u16 = 0x04;
const ATA_LBA_HI: u16 = 0x05;
const ATA_DRIVE_HEAD: u16 = 0x06;
const ATA_STATUS_CMD: u16 = 0x07;

const ATA_STATUS_ERR: u8 = 0x01;
const ATA_STATUS_DRQ: u8 = 0x08;
const ATA_STATUS_BSY: u8 = 0x80;

const ATA_CMD_READ_PIO: u8 = 0x20;
const ATA_CMD_READ_PIO_EXT: u8 = 0x24;
const ATA_CMD_WRITE_PIO: u8 = 0x30;
const ATA_CMD_WRITE_PIO_EXT: u8 = 0x34;
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const ATA_CMD_FLUSH_CACHE: u8 = 0xE7;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;

/// Драйвер блочного накопителя ATA PIO (IDE / PIIX3 / PIIX4)
pub struct AtaDevice {
    base_port: u16,
    is_slave: bool,
    sector_count: u64,
}

impl AtaDevice {
    fn wait_ready(&self) -> Result<(), BlockDeviceError> {
        let mut status_port = Port::<u8>::new(self.base_port + ATA_STATUS_CMD);
        for _ in 0..100_000 {
            let status = unsafe { status_port.read() };
            if (status & ATA_STATUS_BSY) == 0 && (status & ATA_STATUS_DRQ) != 0 {
                return Ok(());
            }
            if (status & ATA_STATUS_ERR) != 0 {
                return Err(BlockDeviceError::IoError);
            }
            core::hint::spin_loop();
        }
        Err(BlockDeviceError::IoError)
    }

    fn wait_not_busy(&self) -> Result<(), BlockDeviceError> {
        let mut status_port = Port::<u8>::new(self.base_port + ATA_STATUS_CMD);
        for _ in 0..100_000 {
            let status = unsafe { status_port.read() };
            if (status & ATA_STATUS_BSY) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(BlockDeviceError::IoError)
    }

    /// Проверяет наличие и идентифицирует жесткий диск ATA
    pub fn probe(base_port: u16, is_slave: bool) -> Option<Self> {
        let mut drive_port = Port::<u8>::new(base_port + ATA_DRIVE_HEAD);
        let mut status_port = Port::<u8>::new(base_port + ATA_STATUS_CMD);
        let mut seccount_port = Port::<u8>::new(base_port + ATA_SECCOUNT);
        let mut lba_lo = Port::<u8>::new(base_port + ATA_LBA_LO);
        let mut lba_mid = Port::<u8>::new(base_port + ATA_LBA_MID);
        let mut lba_hi = Port::<u8>::new(base_port + ATA_LBA_HI);

        let drive_sel = if is_slave { 0xB0 } else { 0xA0 };
        unsafe {
            drive_port.write(drive_sel);
            // Задержка 400 нс для переключения шины
            for _ in 0..4 {
                let _ = status_port.read();
            }

            seccount_port.write(0);
            lba_lo.write(0);
            lba_mid.write(0);
            lba_hi.write(0);

            status_port.write(ATA_CMD_IDENTIFY);
            let status = status_port.read();
            if status == 0 || status == 0xFF {
                return None;
            }

            // ATAPI сигнатура (0xEB14) означает CD-ROM, не обычный диск ATA
            let mid = lba_mid.read();
            let hi = lba_hi.read();
            if mid != 0 || hi != 0 {
                return None;
            }
        }

        let dev = Self {
            base_port,
            is_slave,
            sector_count: 0,
        };

        if dev.wait_ready().is_err() {
            return None;
        }

        let mut data_port = Port::<u16>::new(base_port + ATA_DATA);
        let mut id_buf = [0u16; 256];
        for word in id_buf.iter_mut() {
            *word = unsafe { data_port.read() };
        }

        let lba28_sectors = (id_buf[60] as u32) | ((id_buf[61] as u32) << 16);
        let lba48_supported = (id_buf[83] & (1 << 10)) != 0;
        let sector_count = if lba48_supported {
            let lba48 = (id_buf[100] as u64)
                | ((id_buf[101] as u64) << 16)
                | ((id_buf[102] as u64) << 32)
                | ((id_buf[103] as u64) << 48);
            if lba48 > 0 { lba48 } else { lba28_sectors as u64 }
        } else {
            lba28_sectors as u64
        };

        if sector_count == 0 {
            return None;
        }

        Some(Self {
            base_port,
            is_slave,
            sector_count,
        })
    }
}

impl BlockDevice for AtaDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        let mut data_port = Port::<u16>::new(self.base_port + ATA_DATA);
        let mut seccount_port = Port::<u8>::new(self.base_port + ATA_SECCOUNT);
        let mut lba_lo = Port::<u8>::new(self.base_port + ATA_LBA_LO);
        let mut lba_mid = Port::<u8>::new(self.base_port + ATA_LBA_MID);
        let mut lba_hi = Port::<u8>::new(self.base_port + ATA_LBA_HI);
        let mut drive_port = Port::<u8>::new(self.base_port + ATA_DRIVE_HEAD);
        let mut cmd_port = Port::<u8>::new(self.base_port + ATA_STATUS_CMD);

        for sec in 0..count {
            let lba = start_sector + sec as u64;
            self.wait_not_busy()?;

            unsafe {
                if lba >= 0x1000_0000 {
                    let drive_sel = 0x40 | (if self.is_slave { 0x10 } else { 0x00 });
                    drive_port.write(drive_sel);
                    // Старшие байты (High bytes)
                    seccount_port.write(0);
                    lba_lo.write(((lba >> 24) & 0xFF) as u8);
                    lba_mid.write(((lba >> 32) & 0xFF) as u8);
                    lba_hi.write(((lba >> 40) & 0xFF) as u8);
                    // Младшие байты (Low bytes)
                    seccount_port.write(1);
                    lba_lo.write((lba & 0xFF) as u8);
                    lba_mid.write(((lba >> 8) & 0xFF) as u8);
                    lba_hi.write(((lba >> 16) & 0xFF) as u8);
                    cmd_port.write(ATA_CMD_READ_PIO_EXT);
                } else {
                    let head_sel = 0xE0 | (if self.is_slave { 0x10 } else { 0x00 }) | (((lba >> 24) & 0x0F) as u8);
                    drive_port.write(head_sel);
                    seccount_port.write(1);
                    lba_lo.write((lba & 0xFF) as u8);
                    lba_mid.write(((lba >> 8) & 0xFF) as u8);
                    lba_hi.write(((lba >> 16) & 0xFF) as u8);
                    cmd_port.write(ATA_CMD_READ_PIO);
                }
            }

            self.wait_ready()?;

            let sec_buf = &mut buf[sec * 512..(sec + 1) * 512];
            for chunk in sec_buf.chunks_exact_mut(2) {
                let word = unsafe { data_port.read() };
                chunk.copy_from_slice(&word.to_le_bytes());
            }
        }
        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        let mut data_port = Port::<u16>::new(self.base_port + ATA_DATA);
        let mut seccount_port = Port::<u8>::new(self.base_port + ATA_SECCOUNT);
        let mut lba_lo = Port::<u8>::new(self.base_port + ATA_LBA_LO);
        let mut lba_mid = Port::<u8>::new(self.base_port + ATA_LBA_MID);
        let mut lba_hi = Port::<u8>::new(self.base_port + ATA_LBA_HI);
        let mut drive_port = Port::<u8>::new(self.base_port + ATA_DRIVE_HEAD);
        let mut cmd_port = Port::<u8>::new(self.base_port + ATA_STATUS_CMD);

        for sec in 0..count {
            let lba = start_sector + sec as u64;
            self.wait_not_busy()?;

            unsafe {
                if lba >= 0x1000_0000 {
                    let drive_sel = 0x40 | (if self.is_slave { 0x10 } else { 0x00 });
                    drive_port.write(drive_sel);
                    seccount_port.write(0);
                    lba_lo.write(((lba >> 24) & 0xFF) as u8);
                    lba_mid.write(((lba >> 32) & 0xFF) as u8);
                    lba_hi.write(((lba >> 40) & 0xFF) as u8);
                    seccount_port.write(1);
                    lba_lo.write((lba & 0xFF) as u8);
                    lba_mid.write(((lba >> 8) & 0xFF) as u8);
                    lba_hi.write(((lba >> 16) & 0xFF) as u8);
                    cmd_port.write(ATA_CMD_WRITE_PIO_EXT);
                } else {
                    let head_sel = 0xE0 | (if self.is_slave { 0x10 } else { 0x00 }) | (((lba >> 24) & 0x0F) as u8);
                    drive_port.write(head_sel);
                    seccount_port.write(1);
                    lba_lo.write((lba & 0xFF) as u8);
                    lba_mid.write(((lba >> 8) & 0xFF) as u8);
                    lba_hi.write(((lba >> 16) & 0xFF) as u8);
                    cmd_port.write(ATA_CMD_WRITE_PIO);
                }
            }

            self.wait_ready()?;

            let sec_buf = &buf[sec * 512..(sec + 1) * 512];
            for chunk in sec_buf.chunks_exact(2) {
                let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                unsafe { data_port.write(word); }
            }
        }

        self.wait_not_busy()?;

        let mut status_port = Port::<u8>::new(self.base_port + ATA_STATUS_CMD);
        let status = unsafe { status_port.read() };
        if (status & ATA_STATUS_ERR) != 0 {
            return Err(BlockDeviceError::IoError);
        }

        let flush_cmd = if start_sector >= 0x1000_0000 {
            ATA_CMD_FLUSH_CACHE_EXT
        } else {
            ATA_CMD_FLUSH_CACHE
        };
        unsafe { cmd_port.write(flush_cmd); }
        self.wait_not_busy()
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}
