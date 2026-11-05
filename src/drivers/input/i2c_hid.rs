use alloc::collections::VecDeque;
use crate::drivers::i2c::DesignwareI2c;
use crate::drivers::traits::{InputDevice, InputEvent};

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct I2cHidDescriptor {
    pub hid_desc_length: u16,
    pub bcd_version: u16,
    pub report_desc_length: u16,
    pub report_desc_register: u16,
    pub input_register: u16,
    pub max_input_length: u16,
    pub output_register: u16,
    pub max_output_length: u16,
    pub command_register: u16,
    pub data_register: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_id: u16,
    pub reserved: u32,
}

pub struct I2cTouchpad {
    slave_addr: u8,
    desc: I2cHidDescriptor,
    x: u32,
    y: u32,
    screen_width: u32,
    screen_height: u32,
    button_mask: u32,
    events: VecDeque<InputEvent>,
}

impl I2cTouchpad {
    pub fn probe(
        i2c: &mut DesignwareI2c,
        slave_addr: u8,
        screen_w: u32,
        screen_h: u32,
    ) -> Result<Self, &'static str> {
        // Запрашиваем 30 байт дескриптора HID через стандартный регистр 0x0001
        let reg = [0x01, 0x00];
        let mut desc_raw = [0u8; 30];
        i2c.write(slave_addr, &reg)?;
        i2c.read(slave_addr, &mut desc_raw)?;

        let desc: I2cHidDescriptor = unsafe { core::ptr::read_unaligned(desc_raw.as_ptr() as *const _) };
        let desc_len = desc.hid_desc_length;
        if desc_len != 30 {
            return Err("i2c_hid: invalid descriptor length");
        }

        let vid = desc.vendor_id;
        let pid = desc.product_id;
        let max_input_len = desc.max_input_length;
        let cmd_reg_val = desc.command_register;

        crate::klog!(
            "i2c_hid: detected touchpad VID:{:#06x} PID:{:#06x} (max_input: {} bytes)",
            vid,
            pid,
            max_input_len
        );

        // Отправляем команду Reset (0x01) в Command Register
        let cmd_reg = cmd_reg_val.to_le_bytes();
        let reset_cmd = [cmd_reg[0], cmd_reg[1], 0x00, 0x01]; // Opcode 0x01 = Reset
        let _ = i2c.write(slave_addr, &reset_cmd);

        Ok(Self {
            slave_addr,
            desc,
            x: screen_w / 2,
            y: screen_h / 2,
            screen_width: screen_w,
            screen_height: screen_h,
            button_mask: 0,
            events: VecDeque::with_capacity(64),
        })
    }

    /// Опрашивает тачпад и считывает Input Report со смещениями пальца и касания
    pub fn poll(&mut self, i2c: &mut DesignwareI2c) {
        let mut report_buf = [0u8; 64];
        let max_in = self.desc.max_input_length;
        let len = (max_in as usize).min(64);
        if len < 8 {
            return;
        }

        if i2c.read(self.slave_addr, &mut report_buf[..len]).is_err() {
            return;
        }

        // В протоколе I2C HID первые 2 байта содержат размер пакета (длину)
        let packet_len = u16::from_le_bytes([report_buf[0], report_buf[1]]) as usize;
        if packet_len < 6 || packet_len > len {
            return;
        }

        let tip_switch = (report_buf[2] & 0x01) != 0;
        let raw_x = u16::from_le_bytes([report_buf[3], report_buf[4]]) as u32;
        let raw_y = u16::from_le_bytes([report_buf[5], report_buf[6]]) as u32;

        if tip_switch {
            self.button_mask = 1; // Left button click / touch
            // Масштабируем абсолютные координаты тачпада (0..4095) на разрешение экрана
            self.x = (raw_x * self.screen_width) / 4096;
            self.y = (raw_y * self.screen_height) / 4096;

            if self.events.len() < 64 {
                self.events.push_back(InputEvent {
                    type_: 3, // EV_ABS
                    code: 0, // ABS_X
                    value: self.x,
                });
                self.events.push_back(InputEvent {
                    type_: 3, // EV_ABS
                    code: 1, // ABS_Y
                    value: self.y,
                });
            }
        } else {
            self.button_mask = 0;
        }
    }
}

impl InputDevice for I2cTouchpad {
    fn name(&self) -> &str {
        "i2c-hid-touchpad"
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.x, self.y)
    }

    fn button_mask(&self) -> u32 {
        self.button_mask
    }
}
