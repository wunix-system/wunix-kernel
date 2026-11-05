use crate::drivers::keyboard::push_char;
use crate::drivers::traits::{InputDevice, InputEvent};

/// 8-байтный отчёт USB клавиатуры (Boot Keyboard Protocol)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbKeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keycodes: [u8; 6],
}

impl UsbKeyboardReport {
    /// Декодирует нажатые клавиши USB HID и отправляет символы в очередь ввода ядра
    pub fn process(&self, prev_report: &Self) {
        let is_shift = (self.modifiers & 0x22) != 0; // Left or Right Shift
        let is_ctrl = (self.modifiers & 0x11) != 0;  // Left or Right Ctrl

        for &key in &self.keycodes {
            if key == 0 {
                continue;
            }
            // Проверяем, была ли клавиша уже нажата в прошлом отчёте (защита от дребезга)
            if prev_report.keycodes.contains(&key) {
                continue;
            }

            if let Some(ch) = hid_to_char(key, is_shift, is_ctrl) {
                push_char(ch);
            }
        }
    }
}

/// Трансляция стандартных скан-кодов USB HID (Boot Keyboard) в символы
fn hid_to_char(key: u8, shift: bool, ctrl: bool) -> Option<char> {
    if ctrl {
        return match key {
            0x04..=0x1D => Some((key - 0x04 + 1) as char), // Ctrl+A..Ctrl+Z
            0x2A => Some('\x08'),                         // Backspace
            _ => None,
        };
    }

    match key {
        // 'a'..'z' (0x04..0x1D)
        0x04..=0x1D => {
            let base = if shift { b'A' } else { b'a' };
            Some((base + (key - 0x04)) as char)
        }
        // '1'..'9' (0x1E..0x26), '0' (0x27)
        0x1E => Some(if shift { '!' } else { '1' }),
        0x1F => Some(if shift { '@' } else { '2' }),
        0x20 => Some(if shift { '#' } else { '3' }),
        0x21 => Some(if shift { '$' } else { '4' }),
        0x22 => Some(if shift { '%' } else { '5' }),
        0x23 => Some(if shift { '^' } else { '6' }),
        0x24 => Some(if shift { '&' } else { '7' }),
        0x25 => Some(if shift { '*' } else { '8' }),
        0x26 => Some(if shift { '(' } else { '9' }),
        0x27 => Some(if shift { ')' } else { '0' }),

        0x28 => Some('\r'),                           // Enter
        0x29 => Some('\x1b'),                         // Escape
        0x2A => Some('\x08'),                         // Backspace
        0x2B => Some('\t'),                           // Tab
        0x2C => Some(' '),                            // Space
        0x2D => Some(if shift { '_' } else { '-' }),  // Minus
        0x2E => Some(if shift { '+' } else { '=' }),  // Equals
        0x2F => Some(if shift { '{' } else { '[' }),
        0x30 => Some(if shift { '}' } else { ']' }),
        0x31 => Some(if shift { '|' } else { '\\' }),
        0x33 => Some(if shift { ':' } else { ';' }),
        0x34 => Some(if shift { '"' } else { '\'' }),
        0x36 => Some(if shift { '<' } else { ',' }),
        0x37 => Some(if shift { '>' } else { '.' }),
        0x38 => Some(if shift { '?' } else { '/' }),
        _ => None,
    }
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
