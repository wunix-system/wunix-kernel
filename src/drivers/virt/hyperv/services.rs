use crate::arch::x86_64::cpu::poweroff;
use crate::drivers::virt::hyperv::vmbus::{VmbusChannel, GUID_HEARTBEAT, GUID_KVP, GUID_SHUTDOWN};
use crate::serial_println;

pub struct HyperVHeartbeatService {
    channel: VmbusChannel,
}

impl HyperVHeartbeatService {
    pub fn new(channel_id: u32) -> Result<Self, &'static str> {
        let channel = VmbusChannel::new(channel_id, GUID_HEARTBEAT)?;
        Ok(Self { channel })
    }

    pub fn handle_pulse(&mut self) {
        let mut buf = [0u8; 64];
        if let Ok(Some(len)) = self.channel.receive_packet(&mut buf) {
            if len >= 8 {
                let seq = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let resp_seq = seq.wrapping_add(1);
                let mut resp = [0u8; 8];
                resp[0..4].copy_from_slice(&resp_seq.to_le_bytes());
                let _ = self.channel.send_packet(1, &resp);
            }
        }
    }
}

pub struct HyperVShutdownService {
    channel: VmbusChannel,
}

impl HyperVShutdownService {
    pub fn new(channel_id: u32) -> Result<Self, &'static str> {
        let channel = VmbusChannel::new(channel_id, GUID_SHUTDOWN)?;
        Ok(Self { channel })
    }

    pub fn poll_shutdown(&mut self) {
        let mut buf = [0u8; 64];
        if let Ok(Some(len)) = self.channel.receive_packet(&mut buf) {
            if len >= 4 {
                serial_println!("hyperv: shutdown request received from host, halting system...");
                poweroff();
            }
        }
    }
}

pub struct HyperVKvpService {
    channel: VmbusChannel,
}

impl HyperVKvpService {
    pub fn new(channel_id: u32) -> Result<Self, &'static str> {
        let channel = VmbusChannel::new(channel_id, GUID_KVP)?;
        Ok(Self { channel })
    }

    pub fn report_os_info(&mut self, os_name: &str, os_version: &str) {
        let mut pkt = [0u8; 512];
        let name_bytes = os_name.as_bytes();
        let ver_bytes = os_version.as_bytes();

        let n_len = name_bytes.len().min(128);
        let v_len = ver_bytes.len().min(128);

        pkt[0..n_len].copy_from_slice(&name_bytes[..n_len]);
        pkt[256..256 + v_len].copy_from_slice(&ver_bytes[..v_len]);

        let _ = self.channel.send_packet(1, &pkt);
    }
}
