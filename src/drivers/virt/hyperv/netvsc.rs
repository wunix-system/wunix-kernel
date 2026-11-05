use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::drivers::virt::hyperv::vmbus::{VmbusChannel, GUID_NETVSC};

const RNDIS_MSG_PACKET: u32 = 1;
const RNDIS_MSG_INIT: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RndisPacketMsg {
    msg_type: u32,
    msg_len: u32,
    data_offset: u32,
    data_len: u32,
    oob_data_offset: u32,
    oob_data_len: u32,
    num_oob_elements: u32,
    per_pkt_info_offset: u32,
    per_pkt_info_len: u32,
    vc_handle: u32,
    reserved: u32,
}

/// Синтетический сетевой драйвер Microsoft Hyper-V NetVSC.
pub struct NetVscDevice {
    channel: VmbusChannel,
    mac: [u8; 6],
    req_id: u32,
}

impl NetVscDevice {
    pub fn new(channel_id: u32) -> Result<Self, &'static str> {
        let channel = VmbusChannel::new(channel_id, GUID_NETVSC)?;

        let mut mac = [0x00, 0x15, 0x5D, 0x01, 0x02, 0x03];
        mac[4] = (channel_id & 0xFF) as u8;
        mac[5] = ((channel_id >> 8) & 0xFF) as u8;

        let mut dev = Self {
            channel,
            mac,
            req_id: 1,
        };

        dev.init_rndis()?;
        Ok(dev)
    }

    fn init_rndis(&mut self) -> Result<(), &'static str> {
        let init_msg = [
            RNDIS_MSG_INIT,
            24,
            self.req_id,
            1,
            0,
            1536,
        ];

        let mut raw_bytes = [0u8; 24];
        for (i, &word) in init_msg.iter().enumerate() {
            raw_bytes[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
        }

        self.req_id += 1;
        self.channel.send_packet(1, &raw_bytes)?;
        Ok(())
    }
}

impl NetDevice for NetVscDevice {
    fn driver_name(&self) -> &'static str {
        "hyperv-netvsc"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > 1514 {
            return Err(NetDeviceError::PacketTooLarge);
        }

        let total_rndis_len = core::mem::size_of::<RndisPacketMsg>() + packet.len();
        let mut msg_buf = [0u8; 2048];

        let rndis_hdr = RndisPacketMsg {
            msg_type: RNDIS_MSG_PACKET,
            msg_len: total_rndis_len as u32,
            data_offset: (core::mem::size_of::<RndisPacketMsg>() - 8) as u32,
            data_len: packet.len() as u32,
            oob_data_offset: 0,
            oob_data_len: 0,
            num_oob_elements: 0,
            per_pkt_info_offset: 0,
            per_pkt_info_len: 0,
            vc_handle: 0,
            reserved: 0,
        };

        unsafe {
            core::ptr::copy_nonoverlapping(
                &rndis_hdr as *const _ as *const u8,
                msg_buf.as_mut_ptr(),
                core::mem::size_of::<RndisPacketMsg>(),
            );
            core::ptr::copy_nonoverlapping(
                packet.as_ptr(),
                msg_buf.as_mut_ptr().add(core::mem::size_of::<RndisPacketMsg>()),
                packet.len(),
            );
        }

        self.channel
            .send_packet(1, &msg_buf[..total_rndis_len])
            .map_err(|_| NetDeviceError::IoError)
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let mut recv_buf = [0u8; 2048];
        match self.channel.receive_packet(&mut recv_buf) {
            Ok(Some(bytes_read)) if bytes_read >= core::mem::size_of::<RndisPacketMsg>() => {
                let msg_type = u32::from_le_bytes([recv_buf[0], recv_buf[1], recv_buf[2], recv_buf[3]]);
                if msg_type == RNDIS_MSG_PACKET {
                    let data_offset = u32::from_le_bytes([recv_buf[8], recv_buf[9], recv_buf[10], recv_buf[11]]) as usize;
                    let data_len = u32::from_le_bytes([recv_buf[12], recv_buf[13], recv_buf[14], recv_buf[15]]) as usize;
                    let actual_offset = 8 + data_offset;

                    if actual_offset + data_len <= bytes_read {
                        let to_copy = data_len.min(buf.len());
                        buf[..to_copy].copy_from_slice(&recv_buf[actual_offset..actual_offset + to_copy]);
                        return Ok(Some(to_copy));
                    }
                }
                Ok(None)
            }
            Ok(_) => Ok(None),
            Err(_) => Err(NetDeviceError::IoError),
        }
    }
}
