use alloc::string::String;

pub const FC_TYPE_MGMT: u8 = 0;
pub const FC_TYPE_CTRL: u8 = 1;
pub const FC_TYPE_DATA: u8 = 2;

pub const FC_SUBTYPE_ASSOC_REQ: u8 = 0;
pub const FC_SUBTYPE_ASSOC_RESP: u8 = 1;
pub const FC_SUBTYPE_PROBE_REQ: u8 = 4;
pub const FC_SUBTYPE_PROBE_RESP: u8 = 5;
pub const FC_SUBTYPE_BEACON: u8 = 8;

/// Заголовок фрейма IEEE 802.11 MAC (24 байта)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Dot11Header {
    pub frame_control: u16,
    pub duration_id: u16,
    pub addr1: [u8; 6], // Destination MAC / RA
    pub addr2: [u8; 6], // Source MAC / TA
    pub addr3: [u8; 6], // BSSID
    pub seq_ctrl: u16,
}

impl Dot11Header {
    pub fn frame_type(&self) -> u8 {
        ((self.frame_control >> 2) & 0x03) as u8
    }

    pub fn frame_subtype(&self) -> u8 {
        ((self.frame_control >> 4) & 0x0F) as u8
    }
}

/// Извлекает SSID сети из полезной нагрузки кадра Beacon или Probe Response
pub fn parse_beacon_ssid(payload: &[u8]) -> Option<String> {
    // В полезной нагрузке Beacon первые 12 байт: Timestamp (8) + Beacon Interval (2) + Cap Info (2)
    if payload.len() < 14 {
        return None;
    }

    let mut offset = 12;
    while offset + 2 <= payload.len() {
        let tag_number = payload[offset];
        let tag_len = payload[offset + 1] as usize;
        offset += 2;

        if offset + tag_len > payload.len() {
            break;
        }

        // Тег 0: SSID parameter set
        if tag_number == 0 {
            let ssid_bytes = &payload[offset..offset + tag_len];
            return String::from_utf8(ssid_bytes.to_vec()).ok();
        }

        offset += tag_len;
    }

    None
}
