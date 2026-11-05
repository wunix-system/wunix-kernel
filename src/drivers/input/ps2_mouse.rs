use alloc::collections::VecDeque;
use spin::Mutex;
use x86_64::instructions::port::Port;
use crate::drivers::traits::{InputDevice, InputEvent};

const PS2_DATA_PORT: u16 = 0x60;
const PS2_CMD_PORT: u16 = 0x64;

// Скан-коды и команды мыши i8042
const MOUSE_CMD_RESET: u8 = 0xFF;
const MOUSE_CMD_SET_SAMPLE_RATE: u8 = 0xF3;
const MOUSE_CMD_GET_DEVICE_ID: u8 = 0xF2;
const MOUSE_CMD_ENABLE_DATA: u8 = 0xF4;
const MOUSE_CMD_SET_RESOLUTION: u8 = 0xE8;

pub struct Ps2Mouse {
    is_intellimouse: bool,
    packet_index: usize,
    packet_buf: [u8; 4],
    x: u32,
    y: u32,
    screen_width: u32,
    screen_height: u32,
    button_mask: u32,
    events: VecDeque<InputEvent>,
}

pub static MOUSE_INSTANCE: Mutex<Option<Ps2Mouse>> = Mutex::new(None);

impl Ps2Mouse {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            is_intellimouse: false,
            packet_index: 0,
            packet_buf: [0; 4],
            x: width / 2,
            y: height / 2,
            screen_width: width,
            screen_height: height,
            button_mask: 0,
            events: VecDeque::with_capacity(64),
        }
    }

    /// Обрабатывает один байт, поступивший от контроллера PS/2 мыши (IRQ12)
    pub fn handle_byte(&mut self, byte: u8) {
        // Байт 0 обязан иметь установленный бит 3 (Always 1 в протоколе PS/2).
        if self.packet_index == 0 && (byte & 0x08) == 0 {
            return;
        }

        self.packet_buf[self.packet_index] = byte;
        self.packet_index += 1;

        let expected_len = if self.is_intellimouse { 4 } else { 3 };
        if self.packet_index >= expected_len {
            self.packet_index = 0;
            self.parse_packet();
        }
    }

    fn parse_packet(&mut self) {
        let flags = self.packet_buf[0];
        let mut dx = self.packet_buf[1] as i32;
        let mut dy = self.packet_buf[2] as i32;

        if (flags & 0x10) != 0 {
            dx |= !0xFF;
        }
        if (flags & 0x20) != 0 {
            dy |= !0xFF;
        }

        let new_x = (self.x as i32 + dx).clamp(0, self.screen_width as i32 - 1) as u32;
        let new_y = (self.y as i32 - dy).clamp(0, self.screen_height as i32 - 1) as u32;

        self.x = new_x;
        self.y = new_y;

        let left = (flags & 0x01) != 0;
        let right = (flags & 0x02) != 0;
        let middle = (flags & 0x04) != 0;

        let mut mask = 0;
        if left { mask |= 1; }
        if right { mask |= 2; }
        if middle { mask |= 4; }

        let wheel = if self.is_intellimouse {
            self.packet_buf[3] as i8 as i32
        } else {
            0
        };

        self.button_mask = mask;

        if self.events.len() < 64 {
            self.events.push_back(InputEvent {
                type_: 2, // EV_REL
                code: 0, // REL_X
                value: self.x,
            });
            self.events.push_back(InputEvent {
                type_: 2, // EV_REL
                code: 1, // REL_Y
                value: self.y,
            });
            if wheel != 0 {
                self.events.push_back(InputEvent {
                    type_: 2, // EV_REL
                    code: 8, // REL_WHEEL
                    value: wheel as u32,
                });
            }
        }
    }
}

impl InputDevice for Ps2Mouse {
    fn name(&self) -> &str {
        if self.is_intellimouse {
            "ps2-intellimouse"
        } else {
            "ps2-mouse"
        }
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

unsafe fn wait_write() {
    let mut cmd = Port::<u8>::new(PS2_CMD_PORT);
    for _ in 0..10_000 {
        if unsafe { cmd.read() & 0x02 } == 0 {
            return;
        }
    }
}

unsafe fn wait_read() {
    let mut cmd = Port::<u8>::new(PS2_CMD_PORT);
    for _ in 0..10_000 {
        if unsafe { cmd.read() & 0x01 } != 0 {
            return;
        }
    }
}

unsafe fn send_mouse_cmd(cmd: u8) -> u8 {
    let mut cmd_port = Port::<u8>::new(PS2_CMD_PORT);
    let mut data_port = Port::<u8>::new(PS2_DATA_PORT);

    unsafe {
        wait_write();
        cmd_port.write(0xD4);
        wait_write();
        data_port.write(cmd);
        wait_read();
        data_port.read()
    }
}

pub fn init() {
    let mut mouse = Ps2Mouse::new(1024, 768);

    unsafe {
        let mut cmd_port = Port::<u8>::new(PS2_CMD_PORT);
        let mut data_port = Port::<u8>::new(PS2_DATA_PORT);

        wait_write();
        cmd_port.write(0xA8);

        wait_write();
        cmd_port.write(0x20);
        wait_read();
        let mut config = data_port.read();
        config |= 0x02;
        config &= !0x20;
        wait_write();
        cmd_port.write(0x60);
        wait_write();
        data_port.write(config);

        send_mouse_cmd(MOUSE_CMD_RESET);
        wait_read();
        let _ = data_port.read();
        wait_read();
        let _ = data_port.read();

        send_mouse_cmd(MOUSE_CMD_SET_SAMPLE_RATE);
        send_mouse_cmd(200);
        send_mouse_cmd(MOUSE_CMD_SET_SAMPLE_RATE);
        send_mouse_cmd(100);
        send_mouse_cmd(MOUSE_CMD_SET_SAMPLE_RATE);
        send_mouse_cmd(80);

        send_mouse_cmd(MOUSE_CMD_GET_DEVICE_ID);
        wait_read();
        let dev_id = data_port.read();
        if dev_id == 0x03 || dev_id == 0x04 {
            mouse.is_intellimouse = true;
            crate::klog!("ps2_mouse: Microsoft IntelliMouse detected (ID: {:#x})", dev_id);
        } else {
            crate::klog!("ps2_mouse: Standard 3-button PS/2 mouse detected");
        }

        send_mouse_cmd(MOUSE_CMD_SET_RESOLUTION);
        send_mouse_cmd(0x02);

        send_mouse_cmd(MOUSE_CMD_ENABLE_DATA);
    }

    *MOUSE_INSTANCE.lock() = Some(mouse);
}

pub fn handle_interrupt() {
    let mut data_port = Port::<u8>::new(PS2_DATA_PORT);
    let byte = unsafe { data_port.read() };
    if let Some(ref mut mouse) = *MOUSE_INSTANCE.lock() {
        mouse.handle_byte(byte);
    }
}
