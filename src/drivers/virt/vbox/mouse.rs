use alloc::collections::VecDeque;
use crate::drivers::traits::{InputDevice, InputEvent};
use crate::drivers::virt::vbox::vmmdev::VBOX_GUEST;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

/// Драйвер абсолютного указателя мыши VirtualBox (Mouse Integration).
pub struct VBoxMouseDevice {
    screen_width: u32,
    screen_height: u32,
    last_x: u32,
    last_y: u32,
    buttons: u32,
    events: VecDeque<InputEvent>,
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
            events: VecDeque::with_capacity(32),
        }
    }

    pub fn update(&mut self) {
        if let Some(guest) = VBOX_GUEST.lock().as_mut() {
            if let Ok((raw_x, raw_y, buttons_raw)) = guest.get_mouse_status() {
                let norm_x = ((raw_x.max(0) as u64 * self.screen_width as u64) / 0xFFFF) as u32;
                let norm_y = ((raw_y.max(0) as u64 * self.screen_height as u64) / 0xFFFF) as u32;

                let new_x = norm_x.min(self.screen_width.saturating_sub(1));
                let new_y = norm_y.min(self.screen_height.saturating_sub(1));
                let new_buttons = buttons_raw & 0x07;

                let mut changed = false;

                if new_x != self.last_x || new_y != self.last_y {
                    self.last_x = new_x;
                    self.last_y = new_y;
                    self.events.push_back(InputEvent {
                        type_: EV_ABS,
                        code: ABS_X,
                        value: new_x,
                    });
                    self.events.push_back(InputEvent {
                        type_: EV_ABS,
                        code: ABS_Y,
                        value: new_y,
                    });
                    changed = true;
                }

                if new_buttons != self.buttons {
                    let diff = new_buttons ^ self.buttons;
                    if (diff & 1) != 0 {
                        self.events.push_back(InputEvent {
                            type_: EV_KEY,
                            code: BTN_LEFT,
                            value: (new_buttons & 1) as u32,
                        });
                    }
                    if (diff & 2) != 0 {
                        self.events.push_back(InputEvent {
                            type_: EV_KEY,
                            code: BTN_RIGHT,
                            value: ((new_buttons >> 1) & 1) as u32,
                        });
                    }
                    if (diff & 4) != 0 {
                        self.events.push_back(InputEvent {
                            type_: EV_KEY,
                            code: BTN_MIDDLE,
                            value: ((new_buttons >> 2) & 1) as u32,
                        });
                    }
                    self.buttons = new_buttons;
                    changed = true;
                }

                if changed {
                    self.events.push_back(InputEvent {
                        type_: EV_SYN,
                        code: 0,
                        value: 0,
                    });
                }
            }
        }
    }
}

impl InputDevice for VBoxMouseDevice {
    fn name(&self) -> &str {
        "VirtualBox Absolute Mouse"
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        if self.events.is_empty() {
            self.update();
        }
        self.events.pop_front()
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.last_x, self.last_y)
    }

    fn button_mask(&self) -> u32 {
        self.buttons
    }
}
