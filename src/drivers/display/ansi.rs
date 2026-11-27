use core::fmt::Write;

use super::framebuffer::{DEFAULT_BG_COLOR, DEFAULT_FG_COLOR, MARGIN_X, MARGIN_Y};
use super::{AnsiState, DisplayConsole};
use crate::drivers::font::{draw_char, FONT_HEIGHT, FONT_WIDTH};

const ANSI_NORMAL_COLORS: [u32; 8] = [
    0x000000, 0xAA0000, 0x00AA00, 0xAA5500, 0x0000AA, 0xAA00AA, 0x00AAAA, 0xAAAAAA,
];
const ANSI_BRIGHT_COLORS: [u32; 8] = [
    0x555555, 0xFF5555, 0x55FF55, 0xFFFF55, 0x5599FF, 0xFF55FF, 0x55FFFF, 0xFFFFFF,
];

impl DisplayConsole {
    fn param_or(&self, idx: usize, default: usize) -> usize {
        if idx < self.ansi_param_count && self.ansi_params[idx] > 0 {
            self.ansi_params[idx]
        } else {
            default
        }
    }

    fn apply_sgr(&mut self) {
        if self.ansi_param_count == 0 {
            self.fg_color = DEFAULT_FG_COLOR;
            self.bg_color = DEFAULT_BG_COLOR;
            self.is_inverted = false;
            return;
        }
        let mut is_bold = false;
        for i in 0..self.ansi_param_count {
            match self.ansi_params[i] {
                0 => {
                    self.fg_color = DEFAULT_FG_COLOR;
                    self.bg_color = DEFAULT_BG_COLOR;
                    self.is_inverted = false;
                    is_bold = false;
                }
                1 => is_bold = true,
                7 => self.is_inverted = true,
                27 => self.is_inverted = false,
                30..=37 => {
                    let c = self.ansi_params[i] - 30;
                    self.fg_color = if is_bold { ANSI_BRIGHT_COLORS[c] } else { ANSI_NORMAL_COLORS[c] };
                }
                39 => self.fg_color = DEFAULT_FG_COLOR,
                40..=47 => self.bg_color = ANSI_NORMAL_COLORS[self.ansi_params[i] - 40],
                49 => self.bg_color = DEFAULT_BG_COLOR,
                90..=97 => self.fg_color = ANSI_BRIGHT_COLORS[self.ansi_params[i] - 90],
                100..=107 => self.bg_color = ANSI_BRIGHT_COLORS[self.ansi_params[i] - 100],
                _ => {}
            }
        }
    }

    fn handle_esc(&mut self, c: char) {
        match c {
            '[' => {
                self.ansi_state = AnsiState::Csi;
                self.ansi_params = [0; 8];
                self.ansi_param_count = 0;
                self.ansi_private = false;
            }
            '(' | ')' | '*' | '+' => self.ansi_state = AnsiState::EscCharset,
            'M' => {
                if self.row == self.scroll_top {
                    self.scroll_down();
                } else if self.row > 0 {
                    self.row -= 1;
                }
                self.ansi_state = AnsiState::Normal;
            }
            '7' => {
                self.saved_col = self.col;
                self.saved_row = self.row;
                self.ansi_state = AnsiState::Normal;
            }
            '8' => {
                self.col = self.saved_col.min(self.cols.saturating_sub(1));
                self.row = self.saved_row.min(self.rows.saturating_sub(1));
                self.ansi_state = AnsiState::Normal;
            }
            _ => self.ansi_state = AnsiState::Normal,
        }
    }

    fn send_cursor_report(&self) {
        let mut buf = [0u8; 32];
        struct BufWriter<'a> {
            b: &'a mut [u8],
            p: usize,
        }
        impl<'a> Write for BufWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for byte in s.bytes() {
                    if self.p < self.b.len() {
                        self.b[self.p] = byte;
                        self.p += 1;
                    }
                }
                Ok(())
            }
        }
        let mut w = BufWriter { b: &mut buf, p: 0 };
        let _ = write!(w, "\x1b[{};{}R", self.row + 1, self.col + 1);
        for i in 0..w.p {
            crate::fs::handle::push_stdin_byte(buf[i]);
        }
    }

    fn handle_csi_mode(&mut self, enable: bool) {
        if !self.ansi_private {
            return;
        }
        let p = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
        match p {
            1 => self.cursor_key_mode = enable,
            25 if enable => {
                self.cursor_enabled = true;
                self.draw_cursor();
            }
            25 => {
                self.erase_cursor();
                self.cursor_enabled = false;
            }
            47 | 1047 | 1049 if enable => self.enter_alt_screen(),
            47 | 1047 | 1049 => self.leave_alt_screen(),
            _ => {}
        }
    }

    fn handle_csi(&mut self, c: char) {
        match c {
            '0'..='9' => {
                if self.ansi_param_count == 0 {
                    self.ansi_param_count = 1;
                }
                let idx = self.ansi_param_count - 1;
                if idx < 8 {
                    self.ansi_params[idx] = self.ansi_params[idx] * 10 + (c as usize - '0' as usize);
                }
                return;
            }
            ';' => {
                if self.ansi_param_count < 8 {
                    self.ansi_param_count += 1;
                    self.ansi_params[self.ansi_param_count - 1] = 0;
                }
                return;
            }
            '?' => {
                self.ansi_private = true;
                return;
            }
            'h' => self.handle_csi_mode(true),
            'l' => self.handle_csi_mode(false),
            'J' => {
                let p = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                self.erase_display(p);
            }
            'H' | 'f' => {
                self.row = (self.param_or(0, 1) - 1).min(self.rows.saturating_sub(1));
                self.col = (self.param_or(1, 1) - 1).min(self.cols.saturating_sub(1));
            }
            'A' => self.row = self.row.saturating_sub(self.param_or(0, 1)),
            'B' => self.row = (self.row + self.param_or(0, 1)).min(self.rows.saturating_sub(1)),
            'C' => self.col = (self.col + self.param_or(0, 1)).min(self.cols.saturating_sub(1)),
            'D' => self.col = self.col.saturating_sub(self.param_or(0, 1)),
            'd' => self.row = (self.param_or(0, 1) - 1).min(self.rows.saturating_sub(1)),
            'G' | '`' => self.col = (self.param_or(0, 1) - 1).min(self.cols.saturating_sub(1)),
            'r' => {
                if self.ansi_param_count >= 2 && self.ansi_params[0] > 0 && self.ansi_params[1] > 0 {
                    let top = (self.ansi_params[0] - 1).min(self.rows.saturating_sub(1));
                    let bot = (self.ansi_params[1] - 1).min(self.rows.saturating_sub(1));
                    if top < bot {
                        self.scroll_top = top;
                        self.scroll_bottom = bot;
                    }
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows.saturating_sub(1);
                }
                self.row = 0;
                self.col = 0;
            }
            'n' if self.ansi_param_count > 0 && self.ansi_params[0] == 6 => self.send_cursor_report(),
            'K' => {
                let p = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                let line_y = MARGIN_Y + self.row * FONT_HEIGHT;
                let (_, bg) = self.effective_colors();
                let (from_x, to_x) = match p {
                    1 => (MARGIN_X, MARGIN_X + (self.col + 1) * FONT_WIDTH),
                    2 => (MARGIN_X, MARGIN_X + self.cols * FONT_WIDTH),
                    _ => (MARGIN_X + self.col * FONT_WIDTH, MARGIN_X + self.cols * FONT_WIDTH),
                };
                self.framebuffer.fill_rect(from_x, line_y, to_x, line_y + FONT_HEIGHT, bg);
            }
            'm' => self.apply_sgr(),
            'M' => self.delete_lines(self.param_or(0, 1)),
            'L' => self.insert_lines(self.param_or(0, 1)),
            'P' => self.delete_chars(self.param_or(0, 1)),
            '@' => self.insert_chars(self.param_or(0, 1)),
            'X' => self.erase_chars(self.param_or(0, 1)),
            's' => {
                self.saved_col = self.col;
                self.saved_row = self.row;
            }
            'u' => {
                self.col = self.saved_col.min(self.cols.saturating_sub(1));
                self.row = self.saved_row.min(self.rows.saturating_sub(1));
            }
            _ => {}
        }
        self.ansi_state = AnsiState::Normal;
    }

    pub fn write_char(&mut self, c: char) {
        self.erase_cursor();
        self.write_char_raw(c);
        self.draw_cursor();
    }

    pub fn write_char_raw(&mut self, c: char) {
        match self.ansi_state {
            AnsiState::Normal if c == '\x1b' => {
                self.ansi_state = AnsiState::Esc;
                return;
            }
            AnsiState::Esc => {
                self.handle_esc(c);
                return;
            }
            AnsiState::EscCharset => {
                self.ansi_state = AnsiState::Normal;
                return;
            }
            AnsiState::Csi => {
                self.handle_csi(c);
                return;
            }
            AnsiState::Normal => {}
        }

        match c {
            '\n' => {
                self.col = 0;
                self.advance_row();
            }
            '\r' => self.col = 0,
            '\t' => {
                let next_col = (self.col + 8) & !7;
                let (fg, bg) = self.effective_colors();
                while self.col < next_col && self.col < self.cols {
                    draw_char(
                        &mut self.framebuffer,
                        MARGIN_X + self.col * FONT_WIDTH,
                        MARGIN_Y + self.row * FONT_HEIGHT,
                        ' ',
                        fg,
                        bg,
                    );
                    self.col += 1;
                }
            }
            '\x08' => {
                if self.col > 0 {
                    self.col -= 1;
                }
            }
            '\x00'..='\x1f' | '\x7f' => {}
            ch => {
                if self.col >= self.cols {
                    self.col = 0;
                    self.advance_row();
                }
                let (fg, bg) = self.effective_colors();
                draw_char(
                    &mut self.framebuffer,
                    MARGIN_X + self.col * FONT_WIDTH,
                    MARGIN_Y + self.row * FONT_HEIGHT,
                    ch,
                    fg,
                    bg,
                );
                self.col += 1;
            }
        }
    }

    fn advance_row(&mut self) {
        if self.row == self.scroll_bottom {
            self.scroll_up();
        } else if self.row < self.rows.saturating_sub(1) {
            self.row += 1;
        }
    }
}
