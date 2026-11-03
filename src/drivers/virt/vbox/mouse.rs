use crate::drivers::traits::InputDevice;
use crate::drivers::virt::vbox::vmmdev::VBOX_GUEST;
use crate::drivers::virtio::input::VirtioInputEvent;

/// Драйвер абсолютного указателя мыши VirtualBox (Mouse Integration).
pub struct VBoxMouseDevice {
    screen_width: u32,
    screen_height: u32,
    last_x: u32,
    last_y: u32,
    buttons: u32,
}

impl VBoxMouseDevice {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        if let Some(guest) = VBOX_GUEST.lock().as_mut() {
            let _ = guest.set_mouse_status(true);
        }

        Self {
            screen_width,
            screen_height,
            last_x: screen_width / 2,
            last_y: screen_height / 2,
            buttons: 0,
        }
    }

    pub fn update(&mut self) {
        if let Some(guest) = VBOX_GUEST.lock().as_mut() {
            if let Ok((raw_x, raw_y, _)) = guest.get_mouse_status() {
                let norm_x = ((raw_x.max(0) as u64 * self.screen_width as u64) / 0xFFFF) as u32;
                let norm_y = ((raw_y.max(0) as u64 * self.screen_height as u64) / 0xFFFF) as u32;

                self.last_x = norm_x.min(self.screen_width.saturating_sub(1));
                self.last_y = norm_y.min(self.screen_height.saturating_sub(1));
            }
        }
    }
}

impl InputDevice for VBoxMouseDevice {
    fn name(&self) -> &str {
        "VirtualBox Absolute Mouse"
    }

    fn poll_event(&mut self) -> Option<VirtioInputEvent> {
        self.update();
        None
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.last_x, self.last_y)
    }

    fn button_mask(&self) -> u32 {
        self.buttons
    }
}
