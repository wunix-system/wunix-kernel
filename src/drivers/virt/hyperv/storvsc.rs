use crate::drivers::traits::BlockDevice;
use crate::drivers::virt::hyperv::vmbus::{VmbusChannel, GUID_STORVSC};
use crate::drivers::virtio::VirtioError;

const VSTOR_OP_EXECUTE_SRB: u32 = 1;
const VSTOR_OP_BEGIN_INIT: u32 = 2;
const VSTOR_OP_END_INIT: u32 = 3;

const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_READ_CAPACITY_10: u8 = 0x25;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VstorPacket {
    operation: u32,
    flags: u32,
    status: u32,
    target_id: u8,
    lun_id: u8,
    cdb_len: u8,
    sense_len: u8,
    data_transfer_len: u32,
    cdb: [u8; 16],
}

/// Синтетический SCSI дисковый контроллер Microsoft Hyper-V StorVSC.
pub struct StorVscDevice {
    channel: VmbusChannel,
    sector_count: u64,
    sector_size: usize,
}

impl StorVscDevice {
    pub fn new(channel_id: u32) -> Result<Self, &'static str> {
        let channel = VmbusChannel::new(channel_id, GUID_STORVSC)?;

        let mut dev = Self {
            channel,
            sector_count: 1024 * 1024,
            sector_size: 512,
        };

        dev.init_vstor()?;
        let _ = dev.read_capacity();
        Ok(dev)
    }

    fn init_vstor(&mut self) -> Result<(), &'static str> {
        let mut pkt = VstorPacket {
            operation: VSTOR_OP_BEGIN_INIT,
            ..Default::default()
        };
        let raw = unsafe {
            core::slice::from_raw_parts(&pkt as *const _ as *const u8, core::mem::size_of::<VstorPacket>())
        };
        self.channel.send_packet(1, raw)?;

        pkt.operation = VSTOR_OP_END_INIT;
        let raw_end = unsafe {
            core::slice::from_raw_parts(&pkt as *const _ as *const u8, core::mem::size_of::<VstorPacket>())
        };
        self.channel.send_packet(1, raw_end)?;
        Ok(())
    }

    fn read_capacity(&mut self) -> Result<(), VirtioError> {
        let mut pkt = VstorPacket {
            operation: VSTOR_OP_EXECUTE_SRB,
            cdb_len: 10,
            data_transfer_len: 8,
            ..Default::default()
        };
        pkt.cdb[0] = SCSI_READ_CAPACITY_10;

        let raw = unsafe {
            core::slice::from_raw_parts(&pkt as *const _ as *const u8, core::mem::size_of::<VstorPacket>())
        };
        self.channel.send_packet(1, raw).map_err(|_| VirtioError::IoError)?;

        Ok(())
    }
}

impl BlockDevice for StorVscDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), VirtioError> {
        let num_sectors = (buf.len() / self.sector_size) as u32;

        for s in 0..num_sectors {
            let lba = (start_sector + s as u64) as u32;
            let mut pkt = VstorPacket {
                operation: VSTOR_OP_EXECUTE_SRB,
                cdb_len: 10,
                data_transfer_len: self.sector_size as u32,
                ..Default::default()
            };
            pkt.cdb[0] = SCSI_READ_10;
            pkt.cdb[2] = ((lba >> 24) & 0xFF) as u8;
            pkt.cdb[3] = ((lba >> 16) & 0xFF) as u8;
            pkt.cdb[4] = ((lba >> 8) & 0xFF) as u8;
            pkt.cdb[5] = (lba & 0xFF) as u8;
            pkt.cdb[8] = 1;

            let raw = unsafe {
                core::slice::from_raw_parts(&pkt as *const _ as *const u8, core::mem::size_of::<VstorPacket>())
            };
            self.channel.send_packet(1, raw).map_err(|_| VirtioError::IoError)?;
        }

        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), VirtioError> {
        let num_sectors = (buf.len() / self.sector_size) as u32;

        for s in 0..num_sectors {
            let lba = (start_sector + s as u64) as u32;
            let mut pkt = VstorPacket {
                operation: VSTOR_OP_EXECUTE_SRB,
                cdb_len: 10,
                data_transfer_len: self.sector_size as u32,
                ..Default::default()
            };
            pkt.cdb[0] = SCSI_WRITE_10;
            pkt.cdb[2] = ((lba >> 24) & 0xFF) as u8;
            pkt.cdb[3] = ((lba >> 16) & 0xFF) as u8;
            pkt.cdb[4] = ((lba >> 8) & 0xFF) as u8;
            pkt.cdb[5] = (lba & 0xFF) as u8;
            pkt.cdb[8] = 1;

            let raw = unsafe {
                core::slice::from_raw_parts(&pkt as *const _ as *const u8, core::mem::size_of::<VstorPacket>())
            };
            self.channel.send_packet(1, raw).map_err(|_| VirtioError::IoError)?;
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
