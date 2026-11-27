mod ansi;
pub mod framebuffer;
mod ops;

use alloc::vec::Vec;
use core::fmt;
use spin::Mutex;

pub use framebuffer::{Framebuffer, DEFAULT_BG_COLOR, DEFAULT_FG_COLOR, MARGIN_X, MARGIN_Y};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AnsiState {
    Normal,
    Esc,
    Csi,
    EscCharset,
}

pub struct DisplayConsole {
    pub framebuffer: Framebuffer,
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
    pub fg_color: u32,
    pub bg_color: u32,
    pub is_inverted: bool,
    pub(super) ansi_state: AnsiState,
    pub(super) ansi_params: [usize; 8],
    pub(super) ansi_param_count: usize,
    pub(super) ansi_private: bool,
    pub(super) cursor_drawn: bool,
    pub(super) cursor_col: usize,
    pub(super) cursor_row: usize,
    pub cursor_enabled: bool,
    pub cursor_key_mode: bool,
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    pub saved_col: usize,
    pub saved_row: usize,
    pub(super) in_alt_screen: bool,
    pub(super) alt_saved_fb: Option<Vec<u32>>,
    pub(super) alt_saved_col: usize,
    pub(super) alt_saved_row: usize,
}

unsafe impl Send for DisplayConsole {}
unsafe impl Sync for DisplayConsole {}

impl DisplayConsole {
    pub fn new(fb: &limine::framebuffer::Framebuffer) -> Self {
        let width = fb.width as usize;
        let height = fb.height as usize;
        let pitch = fb.pitch as usize;
        let bpp = fb.bpp;
        let address = fb.address() as *mut u8;

        let cols = (width.saturating_sub(MARGIN_X * 2)) / super::font::FONT_WIDTH;
        let rows = (height.saturating_sub(MARGIN_Y * 2)) / super::font::FONT_HEIGHT;

        Self {
            framebuffer: Framebuffer {
                address,
                width,
                height,
                pitch,
                bpp,
            },
            col: 0,
            row: 0,
            cols,
            rows,
            fg_color: DEFAULT_FG_COLOR,
            bg_color: DEFAULT_BG_COLOR,
            is_inverted: false,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 8],
            ansi_param_count: 0,
            ansi_private: false,
            cursor_drawn: false,
            cursor_col: 0,
            cursor_row: 0,
            cursor_enabled: true,
            cursor_key_mode: false,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            saved_col: 0,
            saved_row: 0,
            in_alt_screen: false,
            alt_saved_fb: None,
            alt_saved_col: 0,
            alt_saved_row: 0,
        }
    }
}

impl fmt::Write for DisplayConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char_raw(c);
        }
        Ok(())
    }
}

pub static CONSOLE: Mutex<Option<DisplayConsole>> = Mutex::new(None);

/// Вывод сырого байтового буфера на дисплей за один батч: один erase + draw курсора на весь buf.
/// Невалидные UTF-8 последовательности заменяются символом '?'.
pub fn write_bytes(buf: &[u8]) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(mut guard) = CONSOLE.try_lock() {
            if let Some(console) = guard.as_mut() {
                console.erase_cursor();
                let mut start = 0;
                while start < buf.len() {
                    match core::str::from_utf8(&buf[start..]) {
                        Ok(valid) => {
                            for c in valid.chars() {
                                console.write_char_raw(c);
                            }
                            break;
                        }
                        Err(err) => {
                            let valid_len = err.valid_up_to();
                            if valid_len > 0 {
                                if let Ok(valid) = core::str::from_utf8(&buf[start..start + valid_len]) {
                                    for c in valid.chars() {
                                        console.write_char_raw(c);
                                    }
                                }
                                start += valid_len;
                            }
                            if let Some(error_len) = err.error_len() {
                                // Невалидная последовательность — заменяем '?' чтобы не мигать побайтово
                                console.write_char_raw('?');
                                start += error_len;
                            } else {
                                break;
                            }
                        }
                    }
                }
                console.draw_cursor();
            }
        }
    });
}

pub fn is_present() -> bool {
    CONSOLE.lock().is_some()
}

/// Размеры активного графического фреймбуфера `(width, height)`, если инициализирован.
pub fn screen_dimensions() -> Option<(u32, u32)> {
    CONSOLE
        .lock()
        .as_ref()
        .map(|c| (c.framebuffer.width as u32, c.framebuffer.height as u32))
}

pub fn init(fb: &limine::framebuffer::Framebuffer) {
    let mut console = DisplayConsole::new(fb);
    console.clear();
    *CONSOLE.lock() = Some(console);
}

pub fn clear_screen() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(console) = CONSOLE.lock().as_mut() {
            console.clear();
        }
    });
}

pub fn is_cursor_key_mode() -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        CONSOLE
            .lock()
            .as_ref()
            .map(|c| c.cursor_key_mode)
            .unwrap_or(false)
    })
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        if let Some(mut guard) = CONSOLE.try_lock() {
            if let Some(console) = guard.as_mut() {
                console.erase_cursor();
                let _ = console.write_fmt(args);
                console.draw_cursor();
            }
        }
    });
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::drivers::display::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
