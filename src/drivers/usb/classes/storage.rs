use crate::drivers::traits::{BlockDevice, BlockDeviceError};

/// Заголовок команды USB Mass Storage (Command Block Wrapper, 31 байт)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct UsbCbw {
    pub signature: u32,       // 0x43425355 ("USBC")
    pub tag: u32,
    pub data_length: u32,
    pub flags: u8,            // Бит 7: 1 = чтение (In), 0 = запись (Out)
    pub lun: u8,
    pub cdb_length: u8,
    pub cdb: [u8; 16],        // SCSI Command Descriptor Block
}

/// Статус завершения команды USB Mass Storage (Command Status Wrapper, 13 байт)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct UsbCsw {
    pub signature: u32,       // 0x53425355 ("USBS")
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,           // 0 = Success, 1 = Failed, 2 = Phase Error
}

/// Блочное устройство USB Mass Storage (флеш-накопитель, внешний диск)
pub struct UsbStorageDevice {
    sector_size: usize,
    sector_count: u64,
}

impl UsbStorageDevice {
    pub fn new(sector_count: u64, sector_size: usize) -> Self {
        Self {
            sector_size: if sector_size == 0 { 512 } else { sector_size },
            sector_count,
        }
    }
}

impl BlockDevice for UsbStorageDevice {
    fn read_sectors(&mut self, _start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        // Заглушка для передачи через BOT пайплайн xHCI
        buf.fill(0);
        Ok(())
    }

    fn write_sectors(&mut self, _start_sector: u64, _buf: &[u8]) -> Result<(), BlockDeviceError> {
        Ok(())
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}
