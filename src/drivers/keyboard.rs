use core::sync::atomic::{AtomicU8, Ordering};
use pc_keyboard::{
    layouts, DecodedKey, HandleControl, KeyCode, KeyState, Keyboard, KeyboardLayout, Modifiers,
    ScancodeSet1,
};
use spin::Mutex;
use x86_64::instructions::interrupts;

// Размер кольцевой очереди символов. 128 байт достаточно, чтобы пользователь
// мог быстро набрать целую команду терминала даже при временной задержке обработки.
pub const KEY_QUEUE_CAPACITY: usize = 128;

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

pub struct RingBuffer<T, const CAP: usize> {
    buffer: [T; CAP],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T: Copy, const CAP: usize> RingBuffer<T, CAP> {
    pub const fn new(default_val: T) -> Self {
        Self {
            buffer: [default_val; CAP],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, item: T) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.buffer[self.tail] = item;
        self.tail = (self.tail + 1) % CAP;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let item = self.buffer[self.head];
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        Some(item)
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len >= CAP
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

pub static KEY_QUEUE: Mutex<RingBuffer<char, KEY_QUEUE_CAPACITY>> =
    Mutex::new(RingBuffer::new('\0'));

// Отслеживание физического состояния клавиш-модификаторов для надежного
// распознавания комбинаций переключения раскладки (Alt+Shift или Ctrl+Shift).
struct ModifierTracker {
    lalt: bool,
    ralt: bool,
    lshift: bool,
    rshift: bool,
    lctrl: bool,
    rctrl: bool,
    combo_candidate: bool,
    combo_consumed: bool,
    layout_notification: Option<Layout>,
}

impl ModifierTracker {
    const fn new() -> Self {
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

    fn shift_pressed(&self) -> bool {
        self.lshift || self.rshift
    }

    fn ctrl_pressed(&self) -> bool {
        self.lctrl || self.rctrl
    }

    fn update(&mut self, code: KeyCode, state: KeyState) {
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
            // Модификатор отпущен: переключаем раскладку ровно в том случае,
            // если между нажатием и отпусканием не нажимались третьи клавиши.
            if !self.combo_consumed {
                let new_layout = toggle_layout();
                self.layout_notification = Some(new_layout);
            }
            self.combo_candidate = false;
            self.combo_consumed = false;
        }
    }
}

static MODIFIER_TRACKER: Mutex<ModifierTracker> = Mutex::new(ModifierTracker::new());

// Забирает событие смены раскладки для отображения в терминале индикатора [RU]/[EN]
pub fn take_layout_change() -> Option<Layout> {
    interrupts::without_interrupts(|| MODIFIER_TRACKER.lock().layout_notification.take())
}

// Преобразование скан-кодов для русской раскладки ЙЦУКЕН
fn map_russian_keycode(
    keycode: KeyCode,
    modifiers: &Modifiers,
    handle_ctrl: HandleControl,
) -> DecodedKey {
    // При удержании Ctrl сочетания вроде Ctrl+C, Ctrl+D транслируются в управляющие ASCII-коды
    if modifiers.is_ctrl() && handle_ctrl == HandleControl::MapLettersToUnicode {
        return layouts::Us104Key.map_keycode(keycode, modifiers, handle_ctrl);
    }

    let is_caps = modifiers.is_shifted() ^ modifiers.capslock;
    let is_shifted = modifiers.is_shifted();

    let ch = match keycode {
        // Цифровой ряд и знаки пунктуации русской раскладки
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

        // Верхний ряд (ЙЦУКЕН)
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

        // Средний ряд (ФЫВАПРОЛДЖЭ)
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

        // Нижний ряд (ЯЧСМИТЬБЮ.)
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

        // Управляющие клавиши ввода
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

static KEYBOARD: Mutex<Keyboard<DualLayout, ScancodeSet1>> = Mutex::new(Keyboard::new(
    ScancodeSet1::new(),
    DualLayout,
    HandleControl::MapLettersToUnicode,
));

pub fn init() {
    interrupts::without_interrupts(|| {
        KEY_QUEUE.lock().clear();
        KEYBOARD.lock().clear();
        *MODIFIER_TRACKER.lock() = ModifierTracker::new();
        // Раскладка по умолчанию всегда английская (US) независимо от локали ядра:
        // шелл и утилиты командной строки ожидают ввод ASCII-команд (ls, cd, cat).
        set_layout(Layout::English);
    });
}

// Вызывается строго из обработчика прерывания IRQ1.
// Не производит аллокаций памяти и вывода на экран (serial/VGA),
// только декодирует скан-код и складывает готовый символ в очередь.
pub fn handle_scancode(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();
    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        let state = key_event.state;
        MODIFIER_TRACKER
            .lock()
            .update(key_event.code, key_event.state);

        let is_ctrl = MODIFIER_TRACKER.lock().ctrl_pressed();
        let is_shift = MODIFIER_TRACKER.lock().shift_pressed();

        // 1. Аппаратная клавиша Enter / Return (включая цифровой блок)
        // Физическая клавиатура ПК генерирует Carriage Return (\r, ASCII 0x0D).
        // В каноническом режиме шелла TTY-драйвер транслирует его в \n благодаря флагу ICRNL.
        // При запуске nano в raw-режиме флаг ICRNL сбрасывается, и редактор ожидает именно \r.
        if state == KeyState::Down
            && (key_event.code == KeyCode::Return || key_event.code == KeyCode::NumpadEnter)
        {
            let mut queue = KEY_QUEUE.lock();
            let _ = queue.push('\r');
            return;
        }

        // 2. Аппаратная клавиша Delete (или Numpad Delete).
        // Внутренний маппинг pc-keyboard по умолчанию преобразует KeyCode::Delete в ASCII 127,
        // из-за чего терминалы и шеллы воспринимают его как Backspace (удаление назад).
        // Отправка стандартной escape-последовательности \x1b[3~ обеспечивает удаление символа ПОД курсором.
        if state == KeyState::Down && key_event.code == KeyCode::Delete {
            let mut queue = KEY_QUEUE.lock();
            for &ch in &['\x1b', '[', '3', '~'] {
                let _ = queue.push(ch);
            }
            return;
        }

        // 3. Аппаратная клавиша Tab (включая обратную табуляцию Shift+Tab).
        // Обычный Tab передает байт 0x09 ('\t').
        // Комбинация Shift+Tab генерирует последовательность ANSI \x1b[Z (CSI Z / Back-Tab),
        // которая позволяет терминалам и шеллам циклически листать подсказки назад.
        if state == KeyState::Down && key_event.code == KeyCode::Tab {
            let mut queue = KEY_QUEUE.lock();
            if is_shift {
                for &ch in &['\x1b', '[', 'Z'] {
                    let _ = queue.push(ch);
                }
            } else {
                let _ = queue.push('\t');
            }
            return;
        }

        // 2. Комбинации Ctrl+Shift (копирование и вставка как в Linux: Ctrl+Shift+C, Ctrl+Shift+V)
        if state == KeyState::Down && is_ctrl && is_shift {
            let seq: Option<&'static [char]> = match key_event.code {
                KeyCode::C => Some(&['\x1b', '[', '9', '9', ';', '6', 'u']),
                KeyCode::V => Some(&['\x1b', '[', '1', '1', '8', ';', '6', 'u']),
                _ => None,
            };
            if let Some(chars) = seq {
                let mut queue = KEY_QUEUE.lock();
                for &ch in chars {
                    let _ = queue.push(ch);
                }
                return;
            }
        }

        // 3. Прямой перехват управляющих комбинаций при зажатом Ctrl (без Shift).
        // Стандартный декодер pc-keyboard в альтернативных раскладках (включая русскую ЙЦУКЕН)
        // может терять модификатор Ctrl или возвращать сырой сканкод вместо управляющего ASCII-байта.
        if state == KeyState::Down && is_ctrl && !is_shift {
            let ctrl_char = match key_event.code {
                KeyCode::A => Some('\x01'), // SOH (Start of heading / начало строки)
                KeyCode::B => Some('\x02'), // STX (Курсор влево)
                KeyCode::C => {
                    if crate::task::syscall::fs::is_isig_enabled() {
                        crate::task::signal::raise_sigint();
                    }
                    Some('\x03')
                }
                KeyCode::D => Some('\x04'), // EOT / EOF
                KeyCode::E => Some('\x05'), // ENQ (Конец строки)
                KeyCode::F => Some('\x06'), // ACK (Курсор вправо)
                KeyCode::G => Some('\x07'), // BEL (nano Help: справка)
                KeyCode::H => Some('\x08'), // BS (Backspace)
                KeyCode::I => Some('\x09'), // HT (Tab)
                KeyCode::J => Some('\x0A'), // LF (nano Justify: выравнивание)
                KeyCode::K => Some('\x0B'), // VT (nano Cut: вырезать строку)
                KeyCode::L => Some('\x0C'), // FF (Clear screen / очистить экран)
                KeyCode::M => Some('\x0D'), // CR (Ввод)
                KeyCode::N => Some('\x0E'), // SO (Следующая строка / история)
                KeyCode::O => Some('\x0F'), // SI (nano WriteOut: сохранить файл)
                KeyCode::P => Some('\x10'), // DLE (Предыдущая строка / история)
                KeyCode::Q => Some('\x11'), // DC1
                KeyCode::R => Some('\x12'), // DC2 (nano Read File: вставить файл)
                KeyCode::S => Some('\x13'), // DC3 (Save: сохранение)
                KeyCode::T => Some('\x14'), // DC4 (nano Execute: запуск команды)
                KeyCode::U => Some('\x15'), // NAK (nano Paste: вставить вырезанное)
                KeyCode::V => Some('\x16'), // SYN (Ctrl+V: листание страницы вниз)
                KeyCode::W => Some('\x17'), // ETB (nano Where Is: поиск)
                KeyCode::X => Some('\x18'), // CAN (nano Exit: выход)
                KeyCode::Y => Some('\x19'), // EM (nano Prev Page: листание вверх)
                KeyCode::Z => Some('\x1A'), // SUB (nano Suspend / Ctrl+Z)
                KeyCode::Spacebar => Some('\x00'), // NUL
                KeyCode::Oem4 => Some('\x1B'),     // ESC (Ctrl+[)
                KeyCode::Oem5 => Some('\x1C'),     // FS (Ctrl+\)
                KeyCode::Oem6 => Some('\x1D'),     // GS (Ctrl+])
                KeyCode::Key6 => Some('\x1E'),     // RS (Ctrl+^)
                KeyCode::OemMinus => Some('\x1F'), // US (Ctrl+_)
                _ => None,
            };
            if let Some(ch) = ctrl_char {
                let mut queue = KEY_QUEUE.lock();
                let _ = queue.push(ch);
                return;
            }
        }

        if let Some(key) = keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => {
                    let mut queue = KEY_QUEUE.lock();
                    let _ = queue.push(character);
                }
                DecodedKey::RawKey(raw_key) => {
                    // Генерируем ANSI escape-последовательности только при нажатии (down),
                    // чтобы интерактивные программы (wsh, editor) реагировали на аппаратные стрелки.
                    if state == KeyState::Down {
                        let is_ctrl = MODIFIER_TRACKER.lock().ctrl_pressed();
                        let is_shift = MODIFIER_TRACKER.lock().shift_pressed();
                        let app_cursor = crate::drivers::display::is_cursor_key_mode();
                        let seq: Option<&'static [char]> = match raw_key {
                            KeyCode::ArrowUp | KeyCode::Numpad8 => {
                                if is_ctrl {
                                    Some(&['\x1b', '[', '1', ';', '5', 'A'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'A'])
                                } else {
                                    Some(&['\x1b', '[', 'A'])
                                }
                            }
                            KeyCode::ArrowDown | KeyCode::Numpad2 => {
                                if is_ctrl {
                                    Some(&['\x1b', '[', '1', ';', '5', 'B'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'B'])
                                } else {
                                    Some(&['\x1b', '[', 'B'])
                                }
                            }
                            KeyCode::ArrowRight | KeyCode::Numpad6 => {
                                if is_ctrl {
                                    Some(&['\x1b', '[', '1', ';', '5', 'C'])
                                } else if is_shift {
                                    Some(&['\x1b', '[', '1', ';', '2', 'C'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'C'])
                                } else {
                                    Some(&['\x1b', '[', 'C'])
                                }
                            }
                            KeyCode::ArrowLeft | KeyCode::Numpad4 => {
                                if is_ctrl {
                                    Some(&['\x1b', '[', '1', ';', '5', 'D'])
                                } else if is_shift {
                                    Some(&['\x1b', '[', '1', ';', '2', 'D'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'D'])
                                } else {
                                    Some(&['\x1b', '[', 'D'])
                                }
                            }
                            KeyCode::Home | KeyCode::Numpad7 => {
                                if is_shift {
                                    Some(&['\x1b', '[', '1', ';', '2', 'H'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'H'])
                                } else {
                                    Some(&['\x1b', '[', 'H'])
                                }
                            }
                            KeyCode::End | KeyCode::Numpad1 => {
                                if is_shift {
                                    Some(&['\x1b', '[', '1', ';', '2', 'F'])
                                } else if app_cursor {
                                    Some(&['\x1b', 'O', 'F'])
                                } else {
                                    Some(&['\x1b', '[', 'F'])
                                }
                            }
                            KeyCode::Delete | KeyCode::NumpadPeriod => Some(&['\x1b', '[', '3', '~']),
                            KeyCode::PageUp | KeyCode::Numpad9 => Some(&['\x1b', '[', '5', '~']),
                            KeyCode::PageDown | KeyCode::Numpad3 => Some(&['\x1b', '[', '6', '~']),
                            _ => None,
                        };
                        if let Some(chars) = seq {
                            let mut queue = KEY_QUEUE.lock();
                            for &ch in chars {
                                let _ = queue.push(ch);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn clear_key_queue() {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().clear());
}

pub fn key_queue_len() -> usize {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().len())
}

// Неблокирующее чтение символа из очереди.
// Отключение прерываний гарантирует защиту от взаимной блокировки (дедлока),
// если в этот момент сработает прерывание IRQ1 на том же ядре.
pub fn pop_char() -> Option<char> {
    interrupts::without_interrupts(|| KEY_QUEUE.lock().pop())
}

// Блокирующее чтение: если очередь пуста, переводит процессор в hlt до прихода прерывания
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

        let key_f = map_russian_keycode(KeyCode::F, &empty_mods, HandleControl::Ignore);
        assert_eq!(key_f, DecodedKey::Unicode('а'));

        let key_shift_f = map_russian_keycode(KeyCode::F, &shift_mods, HandleControl::Ignore);
        assert_eq!(key_shift_f, DecodedKey::Unicode('А'));

        let key_dot = map_russian_keycode(KeyCode::Oem2, &empty_mods, HandleControl::Ignore);
        assert_eq!(key_dot, DecodedKey::Unicode('.'));

        let key_comma = map_russian_keycode(KeyCode::Oem2, &shift_mods, HandleControl::Ignore);
        assert_eq!(key_comma, DecodedKey::Unicode(','));
    }

    #[test_case]
    fn test_return_key_emits_cr() {
        let empty_mods = Modifiers::default();
        let key_return = map_russian_keycode(KeyCode::Return, &empty_mods, HandleControl::Ignore);
        assert_eq!(key_return, DecodedKey::Unicode('\r'));

        clear_key_queue();
        handle_scancode(0x1C); // Scancode 0x1C: Make Enter в ScancodeSet1
        assert_eq!(pop_char(), Some('\r'));
    }
}
