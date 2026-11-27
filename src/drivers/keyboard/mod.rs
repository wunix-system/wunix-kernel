mod layout;
mod ring;

use pc_keyboard::{DecodedKey, HandleControl, KeyCode, KeyState, Keyboard, ScancodeSet1};
use spin::Mutex;
use x86_64::instructions::interrupts;

pub use layout::{active_layout, set_layout, toggle_layout, DualLayout, Layout};
pub use ring::{RingBuffer, KEY_QUEUE_CAPACITY};

use layout::ModifierTracker;

pub static KEY_QUEUE: Mutex<RingBuffer<char, KEY_QUEUE_CAPACITY>> =
    Mutex::new(RingBuffer::new('\0'));

static MODIFIER_TRACKER: Mutex<ModifierTracker> = Mutex::new(ModifierTracker::new());

static KEYBOARD: Mutex<Keyboard<DualLayout, ScancodeSet1>> = Mutex::new(Keyboard::new(
    ScancodeSet1::new(),
    DualLayout,
    HandleControl::MapLettersToUnicode,
));

pub fn take_layout_change() -> Option<Layout> {
    interrupts::without_interrupts(|| MODIFIER_TRACKER.lock().layout_notification.take())
}

pub fn init() {
    interrupts::without_interrupts(|| {
        KEY_QUEUE.lock().clear();
        KEYBOARD.lock().clear();
        *MODIFIER_TRACKER.lock() = ModifierTracker::new();
        // Раскладка по умолчанию всегда английская (US) независимо от локали ядра:
        // шелл и утилиты командной строки ожидают ввод ASCII-команд.
        set_layout(Layout::English);
    });
}

fn push_sequence(chars: &[char]) {
    let mut queue = KEY_QUEUE.lock();
    for &ch in chars {
        let _ = queue.push(ch);
    }
}

fn map_ctrl_byte(code: KeyCode) -> Option<char> {
    match code {
        KeyCode::A => Some('\x01'),
        KeyCode::B => Some('\x02'),
        KeyCode::C => Some('\x03'),
        KeyCode::D => Some('\x04'),
        KeyCode::E => Some('\x05'),
        KeyCode::F => Some('\x06'),
        KeyCode::G => Some('\x07'),
        KeyCode::H => Some('\x08'),
        KeyCode::I => Some('\x09'),
        KeyCode::J => Some('\x0A'),
        KeyCode::K => Some('\x0B'),
        KeyCode::L => Some('\x0C'),
        KeyCode::M => Some('\x0D'),
        KeyCode::N => Some('\x0E'),
        KeyCode::O => Some('\x0F'),
        KeyCode::P => Some('\x10'),
        KeyCode::Q => Some('\x11'),
        KeyCode::R => Some('\x12'),
        KeyCode::S => Some('\x13'),
        KeyCode::T => Some('\x14'),
        KeyCode::U => Some('\x15'),
        KeyCode::V => Some('\x16'),
        KeyCode::W => Some('\x17'),
        KeyCode::X => Some('\x18'),
        KeyCode::Y => Some('\x19'),
        KeyCode::Z => Some('\x1A'),
        KeyCode::Spacebar => Some('\x00'),
        KeyCode::Oem4 => Some('\x1B'),
        KeyCode::Oem5 => Some('\x1C'),
        KeyCode::Oem6 => Some('\x1D'),
        KeyCode::Key6 => Some('\x1E'),
        KeyCode::OemMinus => Some('\x1F'),
        _ => None,
    }
}

fn raw_key_sequence(
    raw_key: KeyCode,
    is_ctrl: bool,
    is_shift: bool,
    app_cursor: bool,
) -> Option<&'static [char]> {
    match raw_key {
        KeyCode::ArrowUp | KeyCode::Numpad8 => Some(if is_ctrl {
            &['\x1b', '[', '1', ';', '5', 'A']
        } else if app_cursor {
            &['\x1b', 'O', 'A']
        } else {
            &['\x1b', '[', 'A']
        }),
        KeyCode::ArrowDown | KeyCode::Numpad2 => Some(if is_ctrl {
            &['\x1b', '[', '1', ';', '5', 'B']
        } else if app_cursor {
            &['\x1b', 'O', 'B']
        } else {
            &['\x1b', '[', 'B']
        }),
        KeyCode::ArrowRight | KeyCode::Numpad6 => Some(if is_ctrl {
            &['\x1b', '[', '1', ';', '5', 'C']
        } else if is_shift {
            &['\x1b', '[', '1', ';', '2', 'C']
        } else if app_cursor {
            &['\x1b', 'O', 'C']
        } else {
            &['\x1b', '[', 'C']
        }),
        KeyCode::ArrowLeft | KeyCode::Numpad4 => Some(if is_ctrl {
            &['\x1b', '[', '1', ';', '5', 'D']
        } else if is_shift {
            &['\x1b', '[', '1', ';', '2', 'D']
        } else if app_cursor {
            &['\x1b', 'O', 'D']
        } else {
            &['\x1b', '[', 'D']
        }),
        KeyCode::Home | KeyCode::Numpad7 => Some(if is_shift {
            &['\x1b', '[', '1', ';', '2', 'H']
        } else if app_cursor {
            &['\x1b', 'O', 'H']
        } else {
            &['\x1b', '[', 'H']
        }),
        KeyCode::End | KeyCode::Numpad1 => Some(if is_shift {
            &['\x1b', '[', '1', ';', '2', 'F']
        } else if app_cursor {
            &['\x1b', 'O', 'F']
        } else {
            &['\x1b', '[', 'F']
        }),
        KeyCode::Delete | KeyCode::NumpadPeriod => Some(&['\x1b', '[', '3', '~']),
        KeyCode::PageUp | KeyCode::Numpad9 => Some(&['\x1b', '[', '5', '~']),
        KeyCode::PageDown | KeyCode::Numpad3 => Some(&['\x1b', '[', '6', '~']),
        _ => None,
    }
}

// Вызывается строго из обработчика прерывания IRQ1 без аллокаций и I/O.
pub fn handle_scancode(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();
    let Ok(Some(key_event)) = keyboard.add_byte(scancode) else {
        return;
    };

    let state = key_event.state;
    let (is_ctrl, is_shift) = {
        let mut tracker = MODIFIER_TRACKER.lock();
        tracker.update(key_event.code, state);
        (tracker.ctrl_pressed(), tracker.shift_pressed())
    };

    if state == KeyState::Down {
        if matches!(key_event.code, KeyCode::Return | KeyCode::NumpadEnter) {
            push_sequence(&['\r']);
            return;
        }
        if key_event.code == KeyCode::Delete {
            push_sequence(&['\x1b', '[', '3', '~']);
            return;
        }
        if key_event.code == KeyCode::Tab {
            push_sequence(if is_shift { &['\x1b', '[', 'Z'] } else { &['\t'] });
            return;
        }
        if is_ctrl && is_shift {
            if let Some(seq) = match key_event.code {
                KeyCode::C => Some(&['\x1b', '[', '9', '9', ';', '6', 'u'] as &[char]),
                KeyCode::V => Some(&['\x1b', '[', '1', '1', '8', ';', '6', 'u'] as &[char]),
                _ => None,
            } {
                push_sequence(seq);
                return;
            }
        }
        if is_ctrl && !is_shift {
            if let Some(ch) = map_ctrl_byte(key_event.code) {
                if !crate::task::syscall::fs::handle_tty_isig_byte(ch as u8) {
                    push_sequence(&[ch]);
                }
                return;
            }
        }
    }

    if let Some(key) = keyboard.process_keyevent(key_event) {
        match key {
            DecodedKey::Unicode(ch) => push_sequence(&[ch]),
            DecodedKey::RawKey(raw) if state == KeyState::Down => {
                let app_cursor = crate::drivers::display::is_cursor_key_mode();
                if let Some(seq) = raw_key_sequence(raw, is_ctrl, is_shift, app_cursor) {
                    push_sequence(seq);
                }
            }
            _ => {}
        }
    }
}

pub fn clear_key_queue() {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().clear());
}

pub fn key_queue_len() -> usize {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().len())
}

pub fn push_char(ch: char) {
    interrupts::without_interrupts(|| {
        let _ = KEY_QUEUE.lock().push(ch);
    });
}

pub fn pop_char() -> Option<char> {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().pop())
}

pub fn read_char() -> char {
    loop {
        if let Some(ch) = pop_char() {
            return ch;
        }
        x86_64::instructions::hlt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout::map_russian_keycode;
    use pc_keyboard::Modifiers;

    #[test_case]
    fn test_ring_buffer_fifo_order() {
        let mut rb = RingBuffer::<char, 4>::new('\0');
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert!(rb.push('a'));
        assert!(rb.push('b'));
        assert!(rb.push('c'));
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.pop(), Some('a'));
        assert_eq!(rb.pop(), Some('b'));
        assert!(rb.push('d'));
        assert!(rb.push('e'));
        assert_eq!(rb.pop(), Some('c'));
        assert_eq!(rb.pop(), Some('d'));
        assert_eq!(rb.pop(), Some('e'));
        assert_eq!(rb.pop(), None);
    }

    #[test_case]
    fn test_ring_buffer_capacity_overflow() {
        let mut rb = RingBuffer::<char, 2>::new('\0');
        assert!(rb.push('x'));
        assert!(rb.push('y'));
        assert!(rb.is_full());
        assert!(!rb.push('z'));
        assert_eq!(rb.pop(), Some('x'));
        assert_eq!(rb.pop(), Some('y'));
        assert_eq!(rb.pop(), None);
    }

    #[test_case]
    fn test_default_layout_is_english() {
        crate::i18n::set_language(crate::i18n::Language::Russian);
        init();
        assert_eq!(active_layout(), Layout::English);
    }

    #[test_case]
    fn test_layout_toggle() {
        set_layout(Layout::English);
        assert_eq!(active_layout(), Layout::English);
        assert_eq!(toggle_layout(), Layout::Russian);
        assert_eq!(active_layout(), Layout::Russian);
        assert_eq!(toggle_layout(), Layout::English);
        assert_eq!(active_layout(), Layout::English);
    }

    #[test_case]
    fn test_russian_mapping_jcuken() {
        let empty_mods = Modifiers::default();
        let mut shift_mods = Modifiers::default();
        shift_mods.lshift = true;

        assert_eq!(
            map_russian_keycode(KeyCode::F, &empty_mods, HandleControl::Ignore),
            DecodedKey::Unicode('а')
        );
        assert_eq!(
            map_russian_keycode(KeyCode::F, &shift_mods, HandleControl::Ignore),
            DecodedKey::Unicode('А')
        );
        assert_eq!(
            map_russian_keycode(KeyCode::Oem2, &empty_mods, HandleControl::Ignore),
            DecodedKey::Unicode('.')
        );
        assert_eq!(
            map_russian_keycode(KeyCode::Oem2, &shift_mods, HandleControl::Ignore),
            DecodedKey::Unicode(',')
        );
    }

    #[test_case]
    fn test_return_key_emits_cr() {
        let empty_mods = Modifiers::default();
        assert_eq!(
            map_russian_keycode(KeyCode::Return, &empty_mods, HandleControl::Ignore),
            DecodedKey::Unicode('\r')
        );
        clear_key_queue();
        handle_scancode(0x1C);
        assert_eq!(pop_char(), Some('\r'));
    }
}
