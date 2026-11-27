use alloc::sync::Arc;
use alloc::vec;
use spin::Mutex;

use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::drivers::usb::host::{UsbHostController, UsbTransferError};

/// Заголовок команды USB Mass Storage Bulk-Only Transport (CBW, 31 байт)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct UsbCbw {
    pub signature: u32, // 0x43425355 ("USBC")
    pub tag: u32,
    pub data_length: u32,
    pub flags: u8, // Бит 7: 1 = чтение (In), 0 = запись (Out)
    pub lun: u8,
    pub cdb_length: u8,
    pub cdb: [u8; 16], // SCSI Command Descriptor Block
}

impl UsbCbw {
    pub fn to_bytes(&self) -> [u8; 31] {
        let mut buf = [0u8; 31];
        buf[0..4].copy_from_slice(&self.signature.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_length.to_le_bytes());
        buf[12] = self.flags;
        buf[13] = self.lun;
        buf[14] = self.cdb_length;
        buf[15..31].copy_from_slice(&self.cdb);
        buf
    }
}

/// Статус завершения команды USB Mass Storage (CSW, 13 байт)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbCsw {
    pub signature: u32, // 0x53425355 ("USBS")
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8, // 0 = Success, 1 = Failed, 2 = Phase Error
}

impl UsbCsw {
    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        if raw.len() < 13 {
            return None;
        }
        Some(Self {
            signature: u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
            tag: u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
            data_residue: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
            status: raw[12],
        })
    }
}

fn map_usb_err(e: UsbTransferError) -> BlockDeviceError {
    match e {
        UsbTransferError::NoDevice => BlockDeviceError::NotReady,
        _ => BlockDeviceError::IoError,
    }
}

/// Блочный накопитель USB Mass Storage поверх реального SCSI Bulk-Only Transport.
pub struct UsbStorageDevice {
    sector_size: usize,
    sector_count: u64,
    slot_id: u8,
    ep_bulk_in: u8,
    ep_bulk_out: u8,
    tag: u32,
    host: Option<Arc<Mutex<dyn UsbHostController>>>,
}

impl UsbStorageDevice {
    pub fn new(slot_id: u8, ep_bulk_in: u8, ep_bulk_out: u8) -> Result<Self, &'static str> {
        Ok(Self {
            sector_size: 512,
            sector_count: 0,
            slot_id,
            ep_bulk_in,
            ep_bulk_out,
            tag: 1,
            host: None,
        })
    }

    pub fn with_host(
        host: Arc<Mutex<dyn UsbHostController>>,
        slot_id: u8,
        ep_bulk_in: u8,
        ep_bulk_out: u8,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            sector_size: 512,
            sector_count: 0,
            slot_id,
            ep_bulk_in,
            ep_bulk_out,
            tag: 1,
            host: Some(host),
        })
    }

    pub fn set_capacity(&mut self, blocks: u64, block_size: usize) {
        self.sector_count = blocks;
        self.sector_size = if block_size == 0 { 512 } else { block_size };
    }

    fn execute_scsi(
        &mut self,
        cdb: &[u8],
        data: &mut [u8],
        is_in: bool,
    ) -> Result<u8, BlockDeviceError> {
        let host = self.host.as_ref().ok_or(BlockDeviceError::NotReady)?.clone();
        let tag = self.tag;
        self.tag = self.tag.wrapping_add(1).max(1);

        let mut cdb_arr = [0u8; 16];
        let cdb_len = cdb.len().min(16);
        cdb_arr[..cdb_len].copy_from_slice(&cdb[..cdb_len]);

        let cbw = UsbCbw {
            signature: 0x4342_5355,
            tag,
            data_length: data.len() as u32,
            flags: if is_in { 0x80 } else { 0x00 },
            lun: 0,
            cdb_length: cdb_len as u8,
            cdb: cdb_arr,
        };
        let mut cbw_buf = cbw.to_bytes();

        let mut ctrl = host.lock();
        ctrl.bulk_transfer(self.slot_id, self.ep_bulk_out, &mut cbw_buf, 1000)
            .map_err(map_usb_err)?;

        if !data.is_empty() {
            let ep = if is_in {
                self.ep_bulk_in
            } else {
                self.ep_bulk_out
            };
            ctrl.bulk_transfer(self.slot_id, ep, data, 2000)
                .map_err(map_usb_err)?;
        }

        let mut csw_buf = [0u8; 13];
        let csw_got = ctrl
            .bulk_transfer(self.slot_id, self.ep_bulk_in, &mut csw_buf, 1000)
            .map_err(map_usb_err)?;
        if csw_got < 13 {
            return Err(BlockDeviceError::IoError);
        }
        let csw = UsbCsw::from_bytes(&csw_buf).ok_or(BlockDeviceError::IoError)?;
        if csw.signature != 0x5342_5355 || csw.tag != tag {
            return Err(BlockDeviceError::IoError);
        }
        Ok(csw.status)
    }

    fn request_sense(&mut self) {
        let mut sense = [0u8; 18];
        let mut cdb = [0u8; 6];
        cdb[0] = 0x03; // REQUEST SENSE
        cdb[4] = sense.len() as u8;
        let _ = self.execute_scsi(&cdb, &mut sense, true);
    }

    /// Инициализация накопителя через SCSI Test Unit Ready, Inquiry и Read Capacity (10).
    pub fn init_storage(&mut self) -> Result<(), &'static str> {
        if self.host.is_none() {
            return Err("usb_storage: host controller is not attached");
        }

        // Первый запрос после сброса часто отвечает Unit Attention; сбрасываем его через TUR + Request Sense.
        for _ in 0..3 {
            let tur = [0u8; 6];
            match self.execute_scsi(&tur, &mut [], true) {
                Ok(0) => break,
                Ok(_) => self.request_sense(),
                Err(_) => self.request_sense(),
            }
        }

        let mut inquiry = [0u8; 36];
        let mut inq_cdb = [0u8; 6];
        inq_cdb[0] = 0x12; // INQUIRY
        inq_cdb[4] = inquiry.len() as u8;
        let _ = self.execute_scsi(&inq_cdb, &mut inquiry, true);

        let mut cap_buf = [0u8; 8];
        let mut cap_cdb = [0u8; 10];
        cap_cdb[0] = 0x25; // READ CAPACITY (10)
        let status = self
            .execute_scsi(&cap_cdb, &mut cap_buf, true)
            .map_err(|_| "usb_storage: READ CAPACITY (10) failed")?;
        if status != 0 {
            self.request_sense();
            let retry = self
                .execute_scsi(&cap_cdb, &mut cap_buf, true)
                .map_err(|_| "usb_storage: READ CAPACITY (10) retry failed")?;
            if retry != 0 {
                return Err("usb_storage: device returned check condition on capacity");
            }
        }

        let last_lba = u32::from_be_bytes([cap_buf[0], cap_buf[1], cap_buf[2], cap_buf[3]]) as u64;
        let blk_size = u32::from_be_bytes([cap_buf[4], cap_buf[5], cap_buf[6], cap_buf[7]]) as usize;

        if blk_size == 0 || blk_size > 4096 {
            return Err("usb_storage: invalid block size reported by device");
        }
        self.sector_size = blk_size;
        self.sector_count = last_lba + 1;
        Ok(())
    }

    fn transfer_blocks(
        &mut self,
        start_sector: u64,
        buf: &mut [u8],
        is_read: bool,
    ) -> Result<(), BlockDeviceError> {
        let sec_sz = self.sector_size;
        if sec_sz == 0 || buf.len() % sec_sz != 0 {
            return Err(BlockDeviceError::IoError);
        }
        let total_sectors = buf.len() / sec_sz;
        if total_sectors == 0 {
            return Ok(());
        }
        if start_sector.saturating_add(total_sectors as u64) > self.sector_count {
            return Err(BlockDeviceError::OutOfBounds);
        }

        // Ограничиваем один SCSI запрос 4 КиБ, чтобы стабильно укладываться в bounce-буфер контроллера.
        let max_per_req = (4096 / sec_sz).max(1);
        let mut done = 0usize;
        while done < total_sectors {
            let chunk_secs = (total_sectors - done).min(max_per_req);
            let lba = (start_sector + done as u64) as u32;
            let blocks = chunk_secs as u16;
            let byte_off = done * sec_sz;
            let byte_len = chunk_secs * sec_sz;

            let mut cdb = [0u8; 10];
            cdb[0] = if is_read { 0x28 } else { 0x2A }; // READ(10) / WRITE(10)
            cdb[2..6].copy_from_slice(&lba.to_be_bytes());
            cdb[7..9].copy_from_slice(&blocks.to_be_bytes());

            let status = self.execute_scsi(&cdb, &mut buf[byte_off..byte_off + byte_len], is_read)?;
            if status != 0 {
                self.request_sense();
                return Err(BlockDeviceError::IoError);
            }
            done += chunk_secs;
        }
        Ok(())
    }
}

impl BlockDevice for UsbStorageDevice {
    fn driver_name(&self) -> &'static str {
        "usb_storage"
    }

    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        self.transfer_blocks(start_sector, buf, true)
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let mut tmp = vec![0u8; buf.len()];
        tmp.copy_from_slice(buf);
        self.transfer_blocks(start_sector, &mut tmp, false)
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}
