use crate::drivers::traits::{InputDevice, InputEvent};

/// 8-байтный отчёт USB клавиатуры (Boot Keyboard Protocol)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbKeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keycodes: [u8; 6],
}

/// 3-байтный отчёт USB мыши (Boot Mouse Protocol)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbMouseReport {
    pub buttons: u8,
    pub x_displacement: i8,
    pub y_displacement: i8,
}

/// Устройство ввода USB Mouse
pub struct UsbMouseDevice {
    cur_x: u32,
    cur_y: u32,
    button_mask: u32,
}

impl UsbMouseDevice {
    pub fn new() -> Self {
        Self {
            cur_x: 512,
            cur_y: 384,
            button_mask: 0,
        }
    }

    pub fn update(&mut self, report: &UsbMouseReport) {
        self.button_mask = (report.buttons & 0x07) as u32;
        self.cur_x = (self.cur_x as i32 + report.x_displacement as i32).clamp(0, 1920) as u32;
        self.cur_y = (self.cur_y as i32 + report.y_displacement as i32).clamp(0, 1080) as u32;
    }
}

impl InputDevice for UsbMouseDevice {
    fn name(&self) -> &str {
        "USB Optical Mouse"
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        None
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.cur_x, self.cur_y)
    }

    fn button_mask(&self) -> u32 {
        self.button_mask
    }
}
