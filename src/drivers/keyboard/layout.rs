use core::sync::atomic::{AtomicU8, Ordering};
use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, KeyState, KeyboardLayout, Modifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Layout {
    English = 0,
    Russian = 1,
}

static CURRENT_LAYOUT: AtomicU8 = AtomicU8::new(Layout::English as u8);

pub fn active_layout() -> Layout {
    match CURRENT_LAYOUT.load(Ordering::Relaxed) {
        0 => Layout::English,
        _ => Layout::Russian,
    }
}

pub fn set_layout(layout: Layout) {
    CURRENT_LAYOUT.store(layout as u8, Ordering::Relaxed);
}

pub fn toggle_layout() -> Layout {
    let next = match active_layout() {
        Layout::English => Layout::Russian,
        Layout::Russian => Layout::English,
    };
    set_layout(next);
    next
}

// Отслеживание физического состояния клавиш-модификаторов для надежного
// распознавания комбинаций переключения раскладки (Alt+Shift или Ctrl+Shift).
pub(super) struct ModifierTracker {
    lalt: bool,
    ralt: bool,
    lshift: bool,
    rshift: bool,
    lctrl: bool,
    rctrl: bool,
    combo_candidate: bool,
    combo_consumed: bool,
    pub(super) layout_notification: Option<Layout>,
}

impl ModifierTracker {
    pub(super) const fn new() -> Self {
        Self {
            lalt: false,
            ralt: false,
            lshift: false,
            rshift: false,
            lctrl: false,
            rctrl: false,
            combo_candidate: false,
            combo_consumed: false,
            layout_notification: None,
        }
    }

    fn alt_pressed(&self) -> bool {
        self.lalt || self.ralt
    }

    pub(super) fn shift_pressed(&self) -> bool {
        self.lshift || self.rshift
    }

    pub(super) fn ctrl_pressed(&self) -> bool {
        self.lctrl || self.rctrl
    }

    pub(super) fn update(&mut self, code: KeyCode, state: KeyState) {
        let is_down = state == KeyState::Down;
        let is_modifier = matches!(
            code,
            KeyCode::LAlt
                | KeyCode::RAltGr
                | KeyCode::LShift
                | KeyCode::RShift
                | KeyCode::LControl
                | KeyCode::RControl
        );

        if !is_modifier {
            if is_down && self.combo_candidate {
                // Если при удержании модификаторов нажата клавиша символа (например, C в Ctrl+Shift+C),
                // аккорд считается частью сочетания клавиш, а не переключением языка.
                self.combo_consumed = true;
            }
            return;
        }

        match code {
            KeyCode::LAlt => self.lalt = is_down,
            KeyCode::RAltGr => self.ralt = is_down,
            KeyCode::LShift => self.lshift = is_down,
            KeyCode::RShift => self.rshift = is_down,
            KeyCode::LControl => self.lctrl = is_down,
            KeyCode::RControl => self.rctrl = is_down,
            _ => {}
        }

        let combo_active = (self.alt_pressed() && self.shift_pressed())
            || (self.ctrl_pressed() && self.shift_pressed());

        if combo_active {
            if !self.combo_candidate {
                self.combo_candidate = true;
                self.combo_consumed = false;
            }
        } else if !is_down && self.combo_candidate {
            // Модификатор отпущен: переключаем раскладку только если не нажималась третья клавиша.
            if !self.combo_consumed {
                let new_layout = toggle_layout();
                self.layout_notification = Some(new_layout);
            }
            self.combo_candidate = false;
            self.combo_consumed = false;
        }
    }
}

// Преобразование скан-кодов для русской раскладки ЙЦУКЕН
pub(super) fn map_russian_keycode(
    keycode: KeyCode,
    modifiers: &Modifiers,
    handle_ctrl: HandleControl,
) -> DecodedKey {
    if modifiers.is_ctrl() && handle_ctrl == HandleControl::MapLettersToUnicode {
        return layouts::Us104Key.map_keycode(keycode, modifiers, handle_ctrl);
    }

    let is_caps = modifiers.is_shifted() ^ modifiers.capslock;
    let is_shifted = modifiers.is_shifted();

    let ch = match keycode {
        KeyCode::Oem8 => if is_caps { 'Ё' } else { 'ё' },
        KeyCode::Key1 => if is_shifted { '!' } else { '1' },
        KeyCode::Key2 => if is_shifted { '"' } else { '2' },
        KeyCode::Key3 => if is_shifted { '№' } else { '3' },
        KeyCode::Key4 => if is_shifted { ';' } else { '4' },
        KeyCode::Key5 => if is_shifted { '%' } else { '5' },
        KeyCode::Key6 => if is_shifted { ':' } else { '6' },
        KeyCode::Key7 => if is_shifted { '?' } else { '7' },
        KeyCode::Key8 => if is_shifted { '*' } else { '8' },
        KeyCode::Key9 => if is_shifted { '(' } else { '9' },
        KeyCode::Key0 => if is_shifted { ')' } else { '0' },
        KeyCode::OemMinus => if is_shifted { '_' } else { '-' },
        KeyCode::OemPlus => if is_shifted { '+' } else { '=' },
        KeyCode::Oem5 => if is_shifted { '/' } else { '\\' },

        KeyCode::Q => if is_caps { 'Й' } else { 'й' },
        KeyCode::W => if is_caps { 'Ц' } else { 'ц' },
        KeyCode::E => if is_caps { 'У' } else { 'у' },
        KeyCode::R => if is_caps { 'К' } else { 'к' },
        KeyCode::T => if is_caps { 'Е' } else { 'е' },
        KeyCode::Y => if is_caps { 'Н' } else { 'н' },
        KeyCode::U => if is_caps { 'Г' } else { 'г' },
        KeyCode::I => if is_caps { 'Ш' } else { 'ш' },
        KeyCode::O => if is_caps { 'Щ' } else { 'щ' },
        KeyCode::P => if is_caps { 'З' } else { 'з' },
        KeyCode::Oem4 => if is_caps { 'Х' } else { 'х' },
        KeyCode::Oem6 => if is_caps { 'Ъ' } else { 'ъ' },

        KeyCode::A => if is_caps { 'Ф' } else { 'ф' },
        KeyCode::S => if is_caps { 'Ы' } else { 'ы' },
        KeyCode::D => if is_caps { 'В' } else { 'в' },
        KeyCode::F => if is_caps { 'А' } else { 'а' },
        KeyCode::G => if is_caps { 'П' } else { 'п' },
        KeyCode::H => if is_caps { 'Р' } else { 'р' },
        KeyCode::J => if is_caps { 'О' } else { 'о' },
        KeyCode::K => if is_caps { 'Л' } else { 'л' },
        KeyCode::L => if is_caps { 'Д' } else { 'д' },
        KeyCode::Oem1 => if is_caps { 'Ж' } else { 'ж' },
        KeyCode::Oem3 => if is_caps { 'Э' } else { 'э' },

        KeyCode::Z => if is_caps { 'Я' } else { 'я' },
        KeyCode::X => if is_caps { 'Ч' } else { 'ч' },
        KeyCode::C => if is_caps { 'С' } else { 'с' },
        KeyCode::V => if is_caps { 'М' } else { 'м' },
        KeyCode::B => if is_caps { 'И' } else { 'и' },
        KeyCode::N => if is_caps { 'Т' } else { 'т' },
        KeyCode::M => if is_caps { 'Ь' } else { 'ь' },
        KeyCode::OemComma => if is_caps { 'Б' } else { 'б' },
        KeyCode::OemPeriod => if is_caps { 'Ю' } else { 'ю' },
        KeyCode::Oem2 => if is_shifted { ',' } else { '.' },

        KeyCode::Spacebar => ' ',
        KeyCode::Return => '\r',
        KeyCode::Backspace => '\x08',
        KeyCode::Tab => '\t',

        other => return layouts::Us104Key.map_keycode(other, modifiers, handle_ctrl),
    };

    DecodedKey::Unicode(ch)
}

pub struct DualLayout;

impl KeyboardLayout for DualLayout {
    fn map_keycode(
        &self,
        keycode: KeyCode,
        modifiers: &Modifiers,
        handle_ctrl: HandleControl,
    ) -> DecodedKey {
        match active_layout() {
            Layout::Russian => map_russian_keycode(keycode, modifiers, handle_ctrl),
            Layout::English => layouts::Us104Key.map_keycode(keycode, modifiers, handle_ctrl),
        }
    }
}
