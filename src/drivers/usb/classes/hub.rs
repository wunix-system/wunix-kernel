pub const USB_CLASS_HUB: u8 = 0x09;

// Hub Class Feature Selectors
pub const HUB_FEAT_C_HUB_LOCAL_POWER: u16 = 0;
pub const HUB_FEAT_C_HUB_OVER_CURRENT: u16 = 1;
pub const PORT_FEAT_CONNECTION: u16 = 0;
pub const PORT_FEAT_ENABLE: u16 = 1;
pub const PORT_FEAT_SUSPEND: u16 = 2;
pub const PORT_FEAT_OVER_CURRENT: u16 = 3;
pub const PORT_FEAT_RESET: u16 = 4;
pub const PORT_FEAT_POWER: u16 = 8;
pub const PORT_FEAT_LOW_SPEED: u16 = 9;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct UsbHubDescriptor {
    pub desc_len: u8,
    pub desc_type: u8, // 0x29
    pub num_ports: u8,
    pub hub_characteristics: u16,
    pub pwr_on_2_pwr_good: u8,
    pub hub_contr_current: u8,
}

pub struct UsbHub {
    pub slot_id: u8,
    pub num_ports: u8,
}

impl UsbHub {
    pub fn new(slot_id: u8, num_ports: u8) -> Self {
        crate::klog!("usb_hub: registered USB Hub on slot {} (ports: {})", slot_id, num_ports);
        Self { slot_id, num_ports }
    }
}
