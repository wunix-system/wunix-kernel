use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::usb::host::UsbHostController;
use crate::i18n::t;

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
        crate::klog!(
            "usb_hub: {} ({}: {}, {}: {})",
            t("зарегистрирован USB-концентратор", "registered USB Hub"),
            t("слот", "slot"),
            slot_id,
            t("портов", "ports"),
            num_ports
        );
        Self { slot_id, num_ports }
    }

    /// Читает реальный Hub Descriptor (0x29) через Class Control Transfer и подаёт питание на порты.
    pub fn probe_with_host(
        host: &Arc<Mutex<dyn UsbHostController>>,
        slot_id: u8,
    ) -> Self {
        let mut desc = [0u8; 9];
        let mut ctrl = host.lock();
        let num_ports = if ctrl
            .control_transfer(slot_id, 0xA0, 6, 0x2900, 0, &mut desc)
            .map(|n| n >= 3 && desc[1] == 0x29)
            .unwrap_or(false)
        {
            desc[2].max(1)
        } else {
            1
        };

        // Включаем питание на всех портах хаба (SET_FEATURE PORT_POWER)
        for port in 1..=num_ports {
            let _ = ctrl.control_transfer(slot_id, 0x23, 3, PORT_FEAT_POWER, port as u16, &mut []);
        }
        drop(ctrl);

        Self::new(slot_id, num_ports)
    }
}
