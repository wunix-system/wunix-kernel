use super::framebuffer::{DEFAULT_BG_COLOR, DEFAULT_FG_COLOR, MARGIN_X, MARGIN_Y};
use super::{AnsiState, DisplayConsole};
use crate::drivers::font::{FONT_HEIGHT, FONT_WIDTH};

impl DisplayConsole {
    pub(super) fn enter_alt_screen(&mut self) {
        if self.in_alt_screen {
            return;
        }
        let pixels = self.framebuffer.width * self.framebuffer.height;
        if pixels > 0 && self.framebuffer.bpp == 32 {
            let mut backup = alloc::vec![0u32; pixels];
            let stride = self.framebuffer.pitch / 4;
            for y in 0..self.framebuffer.height {
                let row_ptr = unsafe { (self.framebuffer.address as *const u32).add(y * stride) };
                let dst = &mut backup[y * self.framebuffer.width..(y + 1) * self.framebuffer.width];
                unsafe {
                    core::ptr::copy_nonoverlapping(row_ptr, dst.as_mut_ptr(), self.framebuffer.width);
                }
            }
            self.alt_saved_fb = Some(backup);
        }
        self.alt_saved_col = self.col;
        self.alt_saved_row = self.row;
        self.in_alt_screen = true;
        self.framebuffer.clear(DEFAULT_BG_COLOR);
        self.col = 0;
        self.row = 0;
    }

    pub(super) fn leave_alt_screen(&mut self) {
        if !self.in_alt_screen {
            return;
        }
        if let Some(backup) = self.alt_saved_fb.take() {
            let stride = self.framebuffer.pitch / 4;
            for y in 0..self.framebuffer.height {
                let row_ptr = unsafe { (self.framebuffer.address as *mut u32).add(y * stride) };
                let src = &backup[y * self.framebuffer.width..(y + 1) * self.framebuffer.width];
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), row_ptr, self.framebuffer.width);
                }
            }
        }
        self.col = self.alt_saved_col.min(self.cols.saturating_sub(1));
        self.row = self.alt_saved_row.min(self.rows.saturating_sub(1));
        self.in_alt_screen = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    pub fn reset_terminal_modes(&mut self) {
        self.leave_alt_screen();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.fg_color = DEFAULT_FG_COLOR;
        self.bg_color = DEFAULT_BG_COLOR;
        self.is_inverted = false;
        self.cursor_enabled = true;
        self.cursor_key_mode = false;
        self.ansi_state = AnsiState::Normal;
    }

    #[inline]
    pub fn effective_colors(&self) -> (u32, u32) {
        if self.is_inverted {
            (self.bg_color, self.fg_color)
        } else {
            (self.fg_color, self.bg_color)
        }
    }

    fn xor_cursor_at(&mut self, col: usize, row: usize) {
        let x_start = MARGIN_X + col * FONT_WIDTH;
        let y_start = MARGIN_Y + row * FONT_HEIGHT;
        let start_y = FONT_HEIGHT.saturating_sub(2);
        for dy in start_y..FONT_HEIGHT {
            let py = y_start + dy;
            if py >= self.framebuffer.height {
                continue;
            }
            for dx in 0..FONT_WIDTH {
                let px = x_start + dx;
                if px >= self.framebuffer.width {
                    continue;
                }
                let offset = py * self.framebuffer.pitch + px * 4;
                unsafe {
                    let ptr = self.framebuffer.address.add(offset) as *mut u32;
                    let cur = core::ptr::read_volatile(ptr);
                    core::ptr::write_volatile(ptr, cur ^ 0x00FFFFFF);
                }
            }
        }
    }

    pub fn draw_cursor(&mut self) {
        if !self.cursor_enabled || self.cursor_drawn || self.col >= self.cols || self.row >= self.rows {
            return;
        }
        self.xor_cursor_at(self.col, self.row);
        self.cursor_col = self.col;
        self.cursor_row = self.row;
        self.cursor_drawn = true;
    }

    pub fn erase_cursor(&mut self) {
        if !self.cursor_drawn {
            return;
        }
        self.xor_cursor_at(self.cursor_col, self.cursor_row);
        self.cursor_drawn = false;
    }

    pub fn clear(&mut self) {
        self.cursor_drawn = false;
        let (_, bg) = self.effective_colors();
        self.framebuffer.clear(bg);
        self.col = 0;
        self.row = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.draw_cursor();
    }

    fn copy_scanline(&self, dst_y: usize, src_y: usize, copy_bytes: usize) {
        if src_y < self.framebuffer.height && dst_y < self.framebuffer.height {
            unsafe {
                let dst = self.framebuffer.address.add(dst_y * self.framebuffer.pitch);
                let src = self.framebuffer.address.add(src_y * self.framebuffer.pitch);
                core::ptr::copy(src, dst, copy_bytes);
            }
        }
    }

    fn fill_row_range(&mut self, start_row: usize, row_count: usize, color: u32) {
        let start_y = MARGIN_Y + start_row * FONT_HEIGHT;
        let end_y = MARGIN_Y + (start_row + row_count) * FONT_HEIGHT;
        self.framebuffer.fill_rect(0, start_y, self.framebuffer.width, end_y, color);
    }

    pub fn scroll_up(&mut self) {
        self.erase_cursor();
        let (top, bot) = (self.scroll_top, self.scroll_bottom);
        if bot <= top {
            return;
        }
        let copy_bytes = self.framebuffer.width * 4;
        let copy_lines = (bot - top) * FONT_HEIGHT;
        let start_dst_y = MARGIN_Y + top * FONT_HEIGHT;
        let start_src_y = MARGIN_Y + (top + 1) * FONT_HEIGHT;
        for y in 0..copy_lines {
            self.copy_scanline(start_dst_y + y, start_src_y + y, copy_bytes);
        }
        let (_, bg) = self.effective_colors();
        self.fill_row_range(bot, 1, bg);
    }

    pub fn scroll_down(&mut self) {
        self.erase_cursor();
        let (top, bot) = (self.scroll_top, self.scroll_bottom);
        if bot <= top {
            return;
        }
        let copy_bytes = self.framebuffer.width * 4;
        let copy_lines = (bot - top) * FONT_HEIGHT;
        for y in (0..copy_lines).rev() {
            self.copy_scanline(
                MARGIN_Y + (top + 1) * FONT_HEIGHT + y,
                MARGIN_Y + top * FONT_HEIGHT + y,
                copy_bytes,
            );
        }
        let (_, bg) = self.effective_colors();
        self.fill_row_range(top, 1, bg);
    }

    pub(super) fn delete_lines(&mut self, count: usize) {
        let (top, bot) = (self.row, self.scroll_bottom);
        if top > bot {
            return;
        }
        let shift = count.min(bot - top + 1);
        let copy_lines = (bot - top + 1 - shift) * FONT_HEIGHT;
        let copy_bytes = self.framebuffer.width * 4;
        for y in 0..copy_lines {
            self.copy_scanline(
                MARGIN_Y + top * FONT_HEIGHT + y,
                MARGIN_Y + (top + shift) * FONT_HEIGHT + y,
                copy_bytes,
            );
        }
        let (_, bg) = self.effective_colors();
        self.fill_row_range(bot + 1 - shift, shift, bg);
        self.col = 0;
    }

    pub(super) fn insert_lines(&mut self, count: usize) {
        let (top, bot) = (self.row, self.scroll_bottom);
        if top > bot {
            return;
        }
        let shift = count.min(bot - top + 1);
        let copy_lines = (bot - top + 1 - shift) * FONT_HEIGHT;
        let copy_bytes = self.framebuffer.width * 4;
        for y in (0..copy_lines).rev() {
            self.copy_scanline(
                MARGIN_Y + (top + shift) * FONT_HEIGHT + y,
                MARGIN_Y + top * FONT_HEIGHT + y,
                copy_bytes,
            );
        }
        let (_, bg) = self.effective_colors();
        self.fill_row_range(top, shift, bg);
        self.col = 0;
    }

    fn shift_cell_pixels(&mut self, py: usize, dst_col: usize, src_col: usize) {
        for dx in 0..FONT_WIDTH {
            let sx = MARGIN_X + src_col * FONT_WIDTH + dx;
            let dx_pos = MARGIN_X + dst_col * FONT_WIDTH + dx;
            if sx < self.framebuffer.width && dx_pos < self.framebuffer.width {
                unsafe {
                    let s_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + sx * 4) as *const u32;
                    let d_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + dx_pos * 4) as *mut u32;
                    core::ptr::write_volatile(d_ptr, core::ptr::read_volatile(s_ptr));
                }
            }
        }
    }

    pub(super) fn delete_chars(&mut self, count: usize) {
        let count = count.min(self.cols.saturating_sub(self.col));
        if count == 0 {
            return;
        }
        let line_y = MARGIN_Y + self.row * FONT_HEIGHT;
        let rem = self.cols - self.col - count;
        for dy in 0..FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                for c in 0..rem {
                    self.shift_cell_pixels(py, self.col + c, self.col + count + c);
                }
            }
        }
        let (_, bg) = self.effective_colors();
        self.framebuffer.fill_rect(
            MARGIN_X + (self.cols - count) * FONT_WIDTH,
            line_y,
            MARGIN_X + self.cols * FONT_WIDTH,
            line_y + FONT_HEIGHT,
            bg,
        );
    }

    pub(super) fn insert_chars(&mut self, count: usize) {
        let count = count.min(self.cols.saturating_sub(self.col));
        if count == 0 {
            return;
        }
        let line_y = MARGIN_Y + self.row * FONT_HEIGHT;
        let rem = self.cols - self.col - count;
        for dy in 0..FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                for c in (0..rem).rev() {
                    self.shift_cell_pixels(py, self.col + count + c, self.col + c);
                }
            }
        }
        let (_, bg) = self.effective_colors();
        self.framebuffer.fill_rect(
            MARGIN_X + self.col * FONT_WIDTH,
            line_y,
            MARGIN_X + (self.col + count) * FONT_WIDTH,
            line_y + FONT_HEIGHT,
            bg,
        );
    }

    pub(super) fn erase_chars(&mut self, count: usize) {
        let line_y = MARGIN_Y + self.row * FONT_HEIGHT;
        let (_, bg) = self.effective_colors();
        self.framebuffer.fill_rect(
            MARGIN_X + self.col * FONT_WIDTH,
            line_y,
            MARGIN_X + (self.col + count).min(self.cols) * FONT_WIDTH,
            line_y + FONT_HEIGHT,
            bg,
        );
    }

    pub(super) fn erase_display(&mut self, param: usize) {
        let (_, bg) = self.effective_colors();
        let line_y = MARGIN_Y + self.row * FONT_HEIGHT;
        let right_x = MARGIN_X + self.cols * FONT_WIDTH;
        match param {
            1 => {
                self.framebuffer.fill_rect(MARGIN_X, MARGIN_Y, right_x, line_y, bg);
                self.framebuffer.fill_rect(
                    MARGIN_X,
                    line_y,
                    MARGIN_X + (self.col + 1).min(self.cols) * FONT_WIDTH,
                    line_y + FONT_HEIGHT,
                    bg,
                );
            }
            2 | 3 => self.framebuffer.clear(bg),
            _ => {
                self.framebuffer.fill_rect(
                    MARGIN_X + self.col * FONT_WIDTH,
                    line_y,
                    right_x,
                    line_y + FONT_HEIGHT,
                    bg,
                );
                self.framebuffer.fill_rect(
                    MARGIN_X,
                    line_y + FONT_HEIGHT,
                    right_x,
                    MARGIN_Y + self.rows * FONT_HEIGHT,
                    bg,
                );
            }
        }
    }
}
