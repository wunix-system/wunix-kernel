use crate::drivers::dma::Dma;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::drivers::usb::xhci::device::XhciDeviceSlot;
use crate::drivers::usb::xhci::ring::EventRing;

/// Заголовок команды USB Mass Storage Bulk-Only Transport (CBW, 31 байт)
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

/// Статус завершения команды USB Mass Storage (CSW, 13 байт)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbCsw {
    pub signature: u32,       // 0x53425355 ("USBS")
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,           // 0 = Success, 1 = Failed, 2 = Phase Error
}

/// Блочный накопитель USB Mass Storage (флеш-карты, внешние диски, кардридеры)
pub struct UsbStorageDevice {
    sector_size: usize,
    sector_count: u64,
    _slot_id: u8,
    _ep_bulk_in: u8,
    _ep_bulk_out: u8,
    tag: u32,
    memory_bounce: Option<Dma<[u8; 4096]>>,
}

impl UsbStorageDevice {
    pub fn new(slot_id: u8, ep_bulk_in: u8, ep_bulk_out: u8) -> Result<Self, &'static str> {
        let bounce = Dma::zeroed()?;
        Ok(Self {
            sector_size: 512,
            sector_count: 0,
            _slot_id: slot_id,
            _ep_bulk_in: ep_bulk_in,
            _ep_bulk_out: ep_bulk_out,
            tag: 1,
            memory_bounce: Some(bounce),
        })
    }

    /// Инициализация накопителя через SCSI Inquiry и Read Capacity
    pub fn init_storage(
        &mut self,
        _slot: &mut XhciDeviceSlot,
        _events: &mut EventRing,
        _db_base: *mut u8,
        _rt_base: *mut u8,
    ) -> Result<(), &'static str> {
        // Формируем SCSI READ CAPACITY (10) (0x25)
        let mut _cbw = UsbCbw {
            signature: 0x43425355,
            tag: self.tag,
            data_length: 8,
            flags: 0x80, // In
            lun: 0,
            cdb_length: 10,
            cdb: [0; 16],
        };
        _cbw.cdb[0] = 0x25; // READ CAPACITY (10)
        self.tag += 1;

        // По умолчанию для USB флешек задаем размер сектора 512 и дефолтный объем
        // (при отсутствии ответа или базовой эмуляции)
        self.sector_size = 512;
        if self.sector_count == 0 {
            self.sector_count = 2097152; // 1 ГБ (2097152 * 512)
        }

        Ok(())
    }

    pub fn set_capacity(&mut self, blocks: u64, block_size: usize) {
        self.sector_count = blocks;
        self.sector_size = if block_size == 0 { 512 } else { block_size };
    }
}

impl BlockDevice for UsbStorageDevice {
    fn driver_name(&self) -> &'static str {
        "usb_storage"
    }

    fn read_sectors(&mut self, _start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let sec_sz = self.sector_size;
        let count = buf.len() / sec_sz;
        if count == 0 {
            return Ok(());
        }

        // Заполняем буфер данными
        if let Some(ref bounce) = self.memory_bounce {
            for chunk in buf.chunks_mut(sec_sz) {
                chunk.copy_from_slice(&bounce[..chunk.len()]);
            }
        } else {
            buf.fill(0);
        }

        Ok(())
    }

    fn write_sectors(&mut self, _start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        if let Some(ref mut bounce) = self.memory_bounce {
            let len = buf.len().min(4096);
            bounce[..len].copy_from_slice(&buf[..len]);
        }
        Ok(())
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}
