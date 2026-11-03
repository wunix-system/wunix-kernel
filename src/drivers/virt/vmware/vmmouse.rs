use crate::drivers::traits::InputDevice;
use crate::drivers::virt::vmware::backdoor::{backdoor_call, VMWARE_CMD_VMMOUSE_COMMAND, VMWARE_CMD_VMMOUSE_DATA, VMWARE_CMD_VMMOUSE_STATUS};
use crate::drivers::virtio::input::VirtioInputEvent;

const VMMOUSE_READ_ID: u32 = 0x4541_4552;
const VMMOUSE_ENABLE: u32 = 0x4542_4545;
const VMMOUSE_DISABLE: u32 = 0x0000_00F5;

/// Драйвер абсолютного указателя мыши VMware (VMMOUSE).
pub struct VmwareMouseDevice {
    screen_width: u32,
    screen_height: u32,
    last_x: u32,
    last_y: u32,
    buttons: u32,
    active: bool,
}

impl VmwareMouseDevice {
    pub fn new(screen_width: u32, screen_height: u32) -> Result<Self, &'static str> {
        let _ = backdoor_call(VMWARE_CMD_VMMOUSE_COMMAND, VMMOUSE_READ_ID);
        let _ = backdoor_call(VMWARE_CMD_VMMOUSE_COMMAND, VMMOUSE_ENABLE);

        Ok(Self {
            screen_width,
            screen_height,
            last_x: screen_width / 2,
            last_y: screen_height / 2,
            buttons: 0,
            active: true,
        })
    }

    pub fn poll(&mut self) {
        if !self.active {
            return;
        }

        let status = backdoor_call(VMWARE_CMD_VMMOUSE_STATUS, 0);
        let num_words = (status.eax & 0xFFFF) as usize;

        if num_words >= 4 {
            let p_flags = backdoor_call(VMWARE_CMD_VMMOUSE_DATA, 1);
            let p_x = backdoor_call(VMWARE_CMD_VMMOUSE_DATA, 1);
            let p_y = backdoor_call(VMWARE_CMD_VMMOUSE_DATA, 1);
            let _p_z = backdoor_call(VMWARE_CMD_VMMOUSE_DATA, 1);

            let raw_x = p_x.eax;
            let raw_y = p_y.eax;

            self.buttons = p_flags.eax & 0x07;

            let norm_x = ((raw_x as u64 * self.screen_width as u64) / 0xFFFF) as u32;
            let norm_y = ((raw_y as u64 * self.screen_height as u64) / 0xFFFF) as u32;

            self.last_x = norm_x.min(self.screen_width.saturating_sub(1));
            self.last_y = norm_y.min(self.screen_height.saturating_sub(1));
        }
    }
}

impl InputDevice for VmwareMouseDevice {
    fn name(&self) -> &str {
        "VMware VMMOUSE Integration"
    }

    fn poll_event(&mut self) -> Option<VirtioInputEvent> {
        self.poll();
        None
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.last_x, self.last_y)
    }

    fn button_mask(&self) -> u32 {
        self.buttons
    }
}

impl Drop for VmwareMouseDevice {
    fn drop(&mut self) {
        let _ = backdoor_call(VMWARE_CMD_VMMOUSE_COMMAND, VMMOUSE_DISABLE);
    }
}
