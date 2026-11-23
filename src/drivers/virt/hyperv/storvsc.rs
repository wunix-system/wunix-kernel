use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::drivers::virt::hyperv::vmbus::{VmbusChannel, GUID_STORVSC};

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

    fn read_capacity(&mut self) -> Result<(), BlockDeviceError> {
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
        self.channel.send_packet(1, raw).map_err(|_| BlockDeviceError::IoError)?;

        let mut resp_buf = [0u8; 128];
        for _ in 0..50_000 {
            if let Ok(Some(n)) = self.channel.receive_packet(&mut resp_buf) {
                if n >= core::mem::size_of::<VstorPacket>() + 8 {
                    let offset = core::mem::size_of::<VstorPacket>();
                    let last_lba = u32::from_be_bytes([
                        resp_buf[offset],
                        resp_buf[offset + 1],
                        resp_buf[offset + 2],
                        resp_buf[offset + 3],
                    ]) as u64;
                    let block_len = u32::from_be_bytes([
                        resp_buf[offset + 4],
                        resp_buf[offset + 5],
                        resp_buf[offset + 6],
                        resp_buf[offset + 7],
                    ]) as usize;
                    if last_lba > 0 {
                        self.sector_count = last_lba + 1;
                    }
                    if block_len > 0 {
                        self.sector_size = block_len;
                    }
                    break;
                }
            }
            core::hint::spin_loop();
        }

        Ok(())
    }
}

impl BlockDevice for StorVscDevice {
    fn driver_name(&self) -> &'static str {
        "hyperv-storvsc"
    }

    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
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
            self.channel.send_packet(1, raw).map_err(|_| BlockDeviceError::IoError)?;

            let mut resp_buf = [0u8; 1024];
            let mut received = false;
            for _ in 0..50_000 {
                if let Ok(Some(n)) = self.channel.receive_packet(&mut resp_buf) {
                    if n >= core::mem::size_of::<VstorPacket>() {
                        let payload_offset = core::mem::size_of::<VstorPacket>();
                        let copy_len = (n.saturating_sub(payload_offset)).min(self.sector_size);
                        let dest = (s as usize) * self.sector_size;
                        if copy_len > 0 {
                            buf[dest..dest + copy_len].copy_from_slice(&resp_buf[payload_offset..payload_offset + copy_len]);
                        }
                        received = true;
                        break;
                    }
                }
                core::hint::spin_loop();
            }

            if !received {
                return Err(BlockDeviceError::IoError);
            }
        }

        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let num_sectors = (buf.len() / self.sector_size) as u32;
        let pkt_size = core::mem::size_of::<VstorPacket>();

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

            let mut req_buf = [0u8; 1024];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &pkt as *const _ as *const u8,
                    req_buf.as_mut_ptr(),
                    pkt_size,
                );
            }

            let src = (s as usize) * self.sector_size;
            req_buf[pkt_size..pkt_size + self.sector_size].copy_from_slice(&buf[src..src + self.sector_size]);

            self.channel
                .send_packet(1, &req_buf[..pkt_size + self.sector_size])
                .map_err(|_| BlockDeviceError::IoError)?;

            let mut resp_buf = [0u8; 128];
            let mut acked = false;
            for _ in 0..50_000 {
                if let Ok(Some(n)) = self.channel.receive_packet(&mut resp_buf) {
                    if n >= pkt_size {
                        acked = true;
                        break;
                    }
                }
                core::hint::spin_loop();
            }

            if !acked {
                return Err(BlockDeviceError::IoError);
            }
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
