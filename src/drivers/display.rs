use core::fmt;
use spin::Mutex;

pub const DEFAULT_FG_COLOR: u32 = 0xFFFFFF;
pub const DEFAULT_BG_COLOR: u32 = 0x000000;

// Отступы терминала от краев экрана для эстетичного визуального вида
pub const MARGIN_X: usize = 12;
pub const MARGIN_Y: usize = 10;

#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub bpp: u16,
}

unsafe impl Send for Framebuffer {}
unsafe impl Sync for Framebuffer {}

impl Framebuffer {
    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            let offset = y * self.pitch + x * 4;
            unsafe {
                core::ptr::write_volatile((self.address.add(offset)) as *mut u32, color);
            }
        }
    }

    pub fn clear(&mut self, color: u32) {
        if color == 0 {
            // Для черного цвета заполняем весь буфер нулями за один вызов
            let total_bytes = self.height * self.pitch;
            unsafe {
                core::ptr::write_bytes(self.address, 0, total_bytes);
            }
        } else {
            for y in 0..self.height {
                for x in 0..self.width {
                    self.put_pixel(x, y, color);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
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
    ansi_state: AnsiState,
    ansi_params: [usize; 8],
    ansi_param_count: usize,
    ansi_private: bool,
    cursor_drawn: bool,
    cursor_col: usize,
    cursor_row: usize,
    pub cursor_enabled: bool,
    pub cursor_key_mode: bool,
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    pub saved_col: usize,
    pub saved_row: usize,
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
        }
    }

    /// Возвращает эффективные цвета переднего плана и фона с учетом инверсии (SGR 7/27)
    #[inline]
    pub fn effective_colors(&self) -> (u32, u32) {
        if self.is_inverted {
            (self.bg_color, self.fg_color)
        } else {
            (self.fg_color, self.bg_color)
        }
    }

    /// Рисует классический аппаратный подчеркивающий курсор (нижние 2 пикселя знакоместа).
    /// Инвертирование пикселей через XOR гарантирует мгновенную обратимость без сохранения фона.
    pub fn draw_cursor(&mut self) {
        if !self.cursor_enabled || self.cursor_drawn {
            return;
        }
        if self.col >= self.cols || self.row >= self.rows {
            return;
        }

        let x_start = MARGIN_X + self.col * super::font::FONT_WIDTH;
        let y_start = MARGIN_Y + self.row * super::font::FONT_HEIGHT;
        let start_y = super::font::FONT_HEIGHT.saturating_sub(2);

        for dy in start_y..super::font::FONT_HEIGHT {
            let py = y_start + dy;
            if py >= self.framebuffer.height {
                continue;
            }
            for dx in 0..super::font::FONT_WIDTH {
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

        self.cursor_col = self.col;
        self.cursor_row = self.row;
        self.cursor_drawn = true;
    }

    /// Стирает отрисованный курсор повторным инвертированием пикселей по сохраненным координатам.
    pub fn erase_cursor(&mut self) {
        if !self.cursor_drawn {
            return;
        }

        let x_start = MARGIN_X + self.cursor_col * super::font::FONT_WIDTH;
        let y_start = MARGIN_Y + self.cursor_row * super::font::FONT_HEIGHT;
        let start_y = super::font::FONT_HEIGHT.saturating_sub(2);

        for dy in start_y..super::font::FONT_HEIGHT {
            let py = y_start + dy;
            if py >= self.framebuffer.height {
                continue;
            }
            for dx in 0..super::font::FONT_WIDTH {
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

    /// Прокрутка строк вверх внутри активной области прокрутки (DECSTBM).
    /// Строки вне диапазона scroll_top..=scroll_bottom (заголовок nano, статус-бар) остаются неподвижными.
    pub fn scroll_up(&mut self) {
        self.erase_cursor();
        let line_height = super::font::FONT_HEIGHT;
        let top = self.scroll_top;
        let bot = self.scroll_bottom;
        if bot <= top {
            return;
        }

        let copy_bytes = self.framebuffer.width * 4;
        let copy_lines = (bot - top) * line_height;

        // Сдвигаем строки с top+1..=bot вверх на top..=bot-1
        for y in 0..copy_lines {
            let dst_y = MARGIN_Y + top * line_height + y;
            let src_y = MARGIN_Y + (top + 1) * line_height + y;
            if src_y < self.framebuffer.height && dst_y < self.framebuffer.height {
                unsafe {
                    let dst = self.framebuffer.address.add(dst_y * self.framebuffer.pitch);
                    let src = self.framebuffer.address.add(src_y * self.framebuffer.pitch);
                    core::ptr::copy(src, dst, copy_bytes);
                }
            }
        }

        // Очищаем освободившуюся нижнюю строку области прокрутки
        let (_, bg) = self.effective_colors();
        let clear_start_y = MARGIN_Y + bot * line_height;
        for y in clear_start_y..(clear_start_y + line_height) {
            if y < self.framebuffer.height {
                let row_ptr = unsafe { self.framebuffer.address.add(y * self.framebuffer.pitch) as *mut u32 };
                for x in 0..self.framebuffer.width {
                    unsafe {
                        core::ptr::write_volatile(row_ptr.add(x), bg);
                    }
                }
            }
        }
    }

    /// Прокрутка строк вниз внутри активной области прокрутки (RI: Reverse Index на верхней границе).
    pub fn scroll_down(&mut self) {
        self.erase_cursor();
        let line_height = super::font::FONT_HEIGHT;
        let top = self.scroll_top;
        let bot = self.scroll_bottom;
        if bot <= top {
            return;
        }

        let copy_bytes = self.framebuffer.width * 4;
        let copy_lines = (bot - top) * line_height;

        // Сдвигаем строки вниз, копируя в обратном порядке
        for y in (0..copy_lines).rev() {
            let dst_y = MARGIN_Y + (top + 1) * line_height + y;
            let src_y = MARGIN_Y + top * line_height + y;
            if src_y < self.framebuffer.height && dst_y < self.framebuffer.height {
                unsafe {
                    let dst = self.framebuffer.address.add(dst_y * self.framebuffer.pitch);
                    let src = self.framebuffer.address.add(src_y * self.framebuffer.pitch);
                    core::ptr::copy(src, dst, copy_bytes);
                }
            }
        }

        // Очищаем верхнюю строку области прокрутки
        let (_, bg) = self.effective_colors();
        let clear_start_y = MARGIN_Y + top * line_height;
        for y in clear_start_y..(clear_start_y + line_height) {
            if y < self.framebuffer.height {
                let row_ptr = unsafe { self.framebuffer.address.add(y * self.framebuffer.pitch) as *mut u32 };
                for x in 0..self.framebuffer.width {
                    unsafe {
                        core::ptr::write_volatile(row_ptr.add(x), bg);
                    }
                }
            }
        }
    }

    /// Удаление строк (CSI M / DL: Delete Line)
    fn delete_lines(&mut self, count: usize) {
        let line_height = super::font::FONT_HEIGHT;
        let top = self.row;
        let bot = self.scroll_bottom;
        if top > bot {
            return;
        }
        let shift = count.min(bot - top + 1);
        let copy_lines = (bot - top + 1 - shift) * line_height;
        let copy_bytes = self.framebuffer.width * 4;
        for y in 0..copy_lines {
            let dst_y = MARGIN_Y + top * line_height + y;
            let src_y = MARGIN_Y + (top + shift) * line_height + y;
            if src_y < self.framebuffer.height && dst_y < self.framebuffer.height {
                unsafe {
                    let dst = self.framebuffer.address.add(dst_y * self.framebuffer.pitch);
                    let src = self.framebuffer.address.add(src_y * self.framebuffer.pitch);
                    core::ptr::copy(src, dst, copy_bytes);
                }
            }
        }
        let (_, bg) = self.effective_colors();
        let clear_start = bot + 1 - shift;
        for r in clear_start..=bot {
            let start_y = MARGIN_Y + r * line_height;
            for y in start_y..(start_y + line_height) {
                if y < self.framebuffer.height {
                    let row_ptr = unsafe { self.framebuffer.address.add(y * self.framebuffer.pitch) as *mut u32 };
                    for x in 0..self.framebuffer.width {
                        unsafe {
                            core::ptr::write_volatile(row_ptr.add(x), bg);
                        }
                    }
                }
            }
        }
        self.col = 0;
    }

    /// Вставка строк (CSI L / IL: Insert Line)
    fn insert_lines(&mut self, count: usize) {
        let line_height = super::font::FONT_HEIGHT;
        let top = self.row;
        let bot = self.scroll_bottom;
        if top > bot {
            return;
        }
        let shift = count.min(bot - top + 1);
        let copy_lines = (bot - top + 1 - shift) * line_height;
        let copy_bytes = self.framebuffer.width * 4;
        for y in (0..copy_lines).rev() {
            let dst_y = MARGIN_Y + (top + shift) * line_height + y;
            let src_y = MARGIN_Y + top * line_height + y;
            if src_y < self.framebuffer.height && dst_y < self.framebuffer.height {
                unsafe {
                    let dst = self.framebuffer.address.add(dst_y * self.framebuffer.pitch);
                    let src = self.framebuffer.address.add(src_y * self.framebuffer.pitch);
                    core::ptr::copy(src, dst, copy_bytes);
                }
            }
        }
        let (_, bg) = self.effective_colors();
        for r in top..(top + shift) {
            let start_y = MARGIN_Y + r * line_height;
            for y in start_y..(start_y + line_height) {
                if y < self.framebuffer.height {
                    let row_ptr = unsafe { self.framebuffer.address.add(y * self.framebuffer.pitch) as *mut u32 };
                    for x in 0..self.framebuffer.width {
                        unsafe {
                            core::ptr::write_volatile(row_ptr.add(x), bg);
                        }
                    }
                }
            }
        }
        self.col = 0;
    }

    /// Удаление символов в текущей строке (CSI P / DCH: Delete Character)
    fn delete_chars(&mut self, count: usize) {
        let count = count.min(self.cols.saturating_sub(self.col));
        if count == 0 {
            return;
        }
        let font_w = super::font::FONT_WIDTH;
        let line_y = MARGIN_Y + self.row * super::font::FONT_HEIGHT;
        let remaining_cols = self.cols - self.col - count;
        for dy in 0..super::font::FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                for c in 0..remaining_cols {
                    let dst_x = MARGIN_X + (self.col + c) * font_w;
                    let src_x = MARGIN_X + (self.col + count + c) * font_w;
                    for dx in 0..font_w {
                        let sx = src_x + dx;
                        let dx_pos = dst_x + dx;
                        if sx < self.framebuffer.width && dx_pos < self.framebuffer.width {
                            unsafe {
                                let s_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + sx * 4) as *const u32;
                                let d_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + dx_pos * 4) as *mut u32;
                                core::ptr::write_volatile(d_ptr, core::ptr::read_volatile(s_ptr));
                            }
                        }
                    }
                }
            }
        }
        let (_, bg) = self.effective_colors();
        for dy in 0..super::font::FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                let start_x = MARGIN_X + (self.cols - count) * font_w;
                let end_x = MARGIN_X + self.cols * font_w;
                for x in start_x..end_x.min(self.framebuffer.width) {
                    self.framebuffer.put_pixel(x, py, bg);
                }
            }
        }
    }

    /// Вставка пустых знакомест в текущей строке (CSI @ / ICH: Insert Character)
    fn insert_chars(&mut self, count: usize) {
        let count = count.min(self.cols.saturating_sub(self.col));
        if count == 0 {
            return;
        }
        let font_w = super::font::FONT_WIDTH;
        let line_y = MARGIN_Y + self.row * super::font::FONT_HEIGHT;
        let remaining_cols = self.cols - self.col - count;
        for dy in 0..super::font::FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                for c in (0..remaining_cols).rev() {
                    let dst_x = MARGIN_X + (self.col + count + c) * font_w;
                    let src_x = MARGIN_X + (self.col + c) * font_w;
                    for dx in (0..font_w).rev() {
                        let sx = src_x + dx;
                        let dx_pos = dst_x + dx;
                        if sx < self.framebuffer.width && dx_pos < self.framebuffer.width {
                            unsafe {
                                let s_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + sx * 4) as *const u32;
                                let d_ptr = self.framebuffer.address.add(py * self.framebuffer.pitch + dx_pos * 4) as *mut u32;
                                core::ptr::write_volatile(d_ptr, core::ptr::read_volatile(s_ptr));
                            }
                        }
                    }
                }
            }
        }
        let (_, bg) = self.effective_colors();
        for dy in 0..super::font::FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                let start_x = MARGIN_X + self.col * font_w;
                let end_x = MARGIN_X + (self.col + count) * font_w;
                for x in start_x..end_x.min(self.framebuffer.width) {
                    self.framebuffer.put_pixel(x, py, bg);
                }
            }
        }
    }

    /// Стирание символов в текущей строке (CSI X / ECH: Erase Character)
    fn erase_chars(&mut self, count: usize) {
        let font_w = super::font::FONT_WIDTH;
        let line_y = MARGIN_Y + self.row * super::font::FONT_HEIGHT;
        let (_, bg) = self.effective_colors();
        let start_x = MARGIN_X + self.col * font_w;
        let end_x = MARGIN_X + (self.col + count).min(self.cols) * font_w;
        for dy in 0..super::font::FONT_HEIGHT {
            let py = line_y + dy;
            if py < self.framebuffer.height {
                for x in start_x..end_x.min(self.framebuffer.width) {
                    self.framebuffer.put_pixel(x, py, bg);
                }
            }
        }
    }

    /// Стирание в дисплее (CSI J / ED: Erase in Display)
    fn erase_display(&mut self, param: usize) {
        let (_, bg) = self.effective_colors();
        let font_w = super::font::FONT_WIDTH;
        let font_h = super::font::FONT_HEIGHT;
        match param {
            1 => {
                // Стираем от начала экрана до текущего курсора включительно
                for r in 0..self.row {
                    let py_start = MARGIN_Y + r * font_h;
                    for dy in 0..font_h {
                        let py = py_start + dy;
                        if py < self.framebuffer.height {
                            for x in MARGIN_X..MARGIN_X + self.cols * font_w {
                                if x < self.framebuffer.width {
                                    self.framebuffer.put_pixel(x, py, bg);
                                }
                            }
                        }
                    }
                }
                let py_start = MARGIN_Y + self.row * font_h;
                let end_x = MARGIN_X + (self.col + 1).min(self.cols) * font_w;
                for dy in 0..font_h {
                    let py = py_start + dy;
                    if py < self.framebuffer.height {
                        for x in MARGIN_X..end_x.min(self.framebuffer.width) {
                            self.framebuffer.put_pixel(x, py, bg);
                        }
                    }
                }
            }
            2 | 3 => {
                // Стираем весь экран без сброса положения курсора
                self.framebuffer.clear(bg);
            }
            _ => {
                // 0 (по умолчанию): стираем от курсора до конца экрана
                let py_start = MARGIN_Y + self.row * font_h;
                let start_x = MARGIN_X + self.col * font_w;
                let end_x = MARGIN_X + self.cols * font_w;
                for dy in 0..font_h {
                    let py = py_start + dy;
                    if py < self.framebuffer.height {
                        for x in start_x..end_x.min(self.framebuffer.width) {
                            self.framebuffer.put_pixel(x, py, bg);
                        }
                    }
                }
                for r in (self.row + 1)..self.rows {
                    let py_start = MARGIN_Y + r * font_h;
                    for dy in 0..font_h {
                        let py = py_start + dy;
                        if py < self.framebuffer.height {
                            for x in MARGIN_X..MARGIN_X + self.cols * font_w {
                                if x < self.framebuffer.width {
                                    self.framebuffer.put_pixel(x, py, bg);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn write_char(&mut self, c: char) {
        self.erase_cursor();
        self.write_char_raw(c);
        self.draw_cursor();
    }

    pub fn write_char_raw(&mut self, c: char) {
        match self.ansi_state {
            AnsiState::Normal => {
                if c == '\x1b' {
                    self.ansi_state = AnsiState::Esc;
                    return;
                }
            }
            AnsiState::Esc => {
                match c {
                    '[' => {
                        self.ansi_state = AnsiState::Csi;
                        self.ansi_params = [0; 8];
                        self.ansi_param_count = 0;
                        self.ansi_private = false;
                    }
                    '(' | ')' | '*' | '+' => {
                        // Обозначение наборов символов VT100 (G0..G3 charset designation, например \x1b(B, \x1b(0)
                        self.ansi_state = AnsiState::EscCharset;
                    }
                    'M' => {
                        // RI: Reverse Index (перемещение строки вверх или прокрутка вниз на верхней границе)
                        if self.row == self.scroll_top {
                            self.scroll_down();
                        } else if self.row > 0 {
                            self.row -= 1;
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    '7' => {
                        // DEC сохранение координат курсора
                        self.saved_col = self.col;
                        self.saved_row = self.row;
                        self.ansi_state = AnsiState::Normal;
                    }
                    '8' => {
                        // DEC восстановление координат курсора
                        self.col = self.saved_col.min(self.cols.saturating_sub(1));
                        self.row = self.saved_row.min(self.rows.saturating_sub(1));
                        self.ansi_state = AnsiState::Normal;
                    }
                    '=' | '>' => {
                        // DECKPAM / DECKPNM (Application / Numeric keypad mode)
                        self.ansi_state = AnsiState::Normal;
                    }
                    _ => {
                        self.ansi_state = AnsiState::Normal;
                    }
                }
                return;
            }
            AnsiState::EscCharset => {
                // Поглощаем идентификатор набора символов ('B', '0' и др.)
                self.ansi_state = AnsiState::Normal;
                return;
            }
            AnsiState::Csi => {
                match c {
                    '0'..='9' => {
                        if self.ansi_param_count == 0 {
                            self.ansi_param_count = 1;
                        }
                        let idx = self.ansi_param_count - 1;
                        if idx < 8 {
                            self.ansi_params[idx] = self.ansi_params[idx] * 10 + (c as usize - '0' as usize);
                        }
                    }
                    ';' => {
                        if self.ansi_param_count < 8 {
                            self.ansi_param_count += 1;
                            self.ansi_params[self.ansi_param_count - 1] = 0;
                        }
                    }
                    '?' => {
                        self.ansi_private = true;
                    }
                    'h' => {
                        if self.ansi_private {
                            let p = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                            if p == 1 {
                                // DECCKM: Application Cursor Keys mode
                                self.cursor_key_mode = true;
                            } else if p == 25 {
                                self.cursor_enabled = true;
                                self.draw_cursor();
                            }
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    'l' => {
                        if self.ansi_private {
                            let p = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                            if p == 1 {
                                // DECCKM: Normal Cursor Keys mode
                                self.cursor_key_mode = false;
                            } else if p == 25 {
                                self.erase_cursor();
                                self.cursor_enabled = false;
                            }
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    'J' => {
                        let param = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                        self.erase_display(param);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'H' | 'f' => {
                        // В ANSI/VT100 координаты 1-based. Если параметры опущены или 0, то по спецификации переходим в (1, 1).
                        let target_row = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            (self.ansi_params[0] - 1).min(self.rows.saturating_sub(1))
                        } else {
                            0
                        };
                        let target_col = if self.ansi_param_count > 1 && self.ansi_params[1] > 0 {
                            (self.ansi_params[1] - 1).min(self.cols.saturating_sub(1))
                        } else {
                            0
                        };
                        self.row = target_row;
                        self.col = target_col;
                        self.ansi_state = AnsiState::Normal;
                    }
                    'A' => {
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.row = self.row.saturating_sub(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'B' => {
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.row = (self.row + count).min(self.rows.saturating_sub(1));
                        self.ansi_state = AnsiState::Normal;
                    }
                    'd' => {
                        // VPA: Line Position Absolute (вертикальное позиционирование курсора)
                        let target_row = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            (self.ansi_params[0] - 1).min(self.rows.saturating_sub(1))
                        } else {
                            0
                        };
                        self.row = target_row;
                        self.ansi_state = AnsiState::Normal;
                    }
                    'G' | '`' => {
                        // CHA / HPA: Character Position Absolute (горизонтальное позиционирование курсора)
                        let target_col = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            (self.ansi_params[0] - 1).min(self.cols.saturating_sub(1))
                        } else {
                            0
                        };
                        self.col = target_col;
                        self.ansi_state = AnsiState::Normal;
                    }
                    'r' => {
                        // DECSTBM: Set Top and Bottom Margins (1-based координаты)
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
                        self.ansi_state = AnsiState::Normal;
                    }
                    'n' => {
                        // DSR: Device Status Report. Запрос 6n (CPR) требует ответа с координатами курсора: ESC [ row ; col R
                        if self.ansi_param_count > 0 && self.ansi_params[0] == 6 {
                            let mut buf = [0u8; 32];
                            let r = self.row + 1;
                            let c = self.col + 1;
                            use core::fmt::Write;
                            struct BufWriter<'a> {
                                b: &'a mut [u8],
                                p: usize,
                            }
                            impl<'a> core::fmt::Write for BufWriter<'a> {
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
                            let _ = write!(w, "\x1b[{};{}R", r, c);
                            for i in 0..w.p {
                                crate::fs::handle::push_stdin_byte(buf[i]);
                            }
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    'K' => {
                        // EL: Erase in Line (стирание строки)
                        let param = if self.ansi_param_count > 0 { self.ansi_params[0] } else { 0 };
                        let line_y = MARGIN_Y + self.row * super::font::FONT_HEIGHT;
                        let (_, bg) = self.effective_colors();
                        let (from_x, to_x) = match param {
                            1 => (MARGIN_X, MARGIN_X + (self.col + 1) * super::font::FONT_WIDTH),
                            2 => (MARGIN_X, MARGIN_X + self.cols * super::font::FONT_WIDTH),
                            _ => (MARGIN_X + self.col * super::font::FONT_WIDTH, MARGIN_X + self.cols * super::font::FONT_WIDTH),
                        };
                        for y in 0..super::font::FONT_HEIGHT {
                            let py = line_y + y;
                            if py < self.framebuffer.height {
                                for x in from_x..to_x.min(self.framebuffer.width) {
                                    self.framebuffer.put_pixel(x, py, bg);
                                }
                            }
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    'D' => {
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.col = self.col.saturating_sub(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'C' => {
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.col = (self.col + count).min(self.cols.saturating_sub(1));
                        self.ansi_state = AnsiState::Normal;
                    }
                    'm' => {
                        if self.ansi_param_count == 0 {
                            self.fg_color = DEFAULT_FG_COLOR;
                            self.bg_color = DEFAULT_BG_COLOR;
                            self.is_inverted = false;
                        } else {
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
                                    30 => self.fg_color = if is_bold { 0x555555 } else { 0x000000 },
                                    31 => self.fg_color = if is_bold { 0xFF5555 } else { 0xAA0000 },
                                    32 => self.fg_color = if is_bold { 0x55FF55 } else { 0x00AA00 },
                                    33 => self.fg_color = if is_bold { 0xFFFF55 } else { 0xAA5500 },
                                    34 => self.fg_color = if is_bold { 0x5599FF } else { 0x0000AA },
                                    35 => self.fg_color = if is_bold { 0xFF55FF } else { 0xAA00AA },
                                    36 => self.fg_color = if is_bold { 0x55FFFF } else { 0x00AAAA },
                                    37 => self.fg_color = if is_bold { 0xFFFFFF } else { 0xAAAAAA },
                                    39 => self.fg_color = DEFAULT_FG_COLOR,
                                    40 => self.bg_color = 0x000000,
                                    41 => self.bg_color = 0xAA0000,
                                    42 => self.bg_color = 0x00AA00,
                                    43 => self.bg_color = 0xAA5500,
                                    44 => self.bg_color = 0x0000AA,
                                    45 => self.bg_color = 0xAA00AA,
                                    46 => self.bg_color = 0x00AAAA,
                                    47 => self.bg_color = 0xAAAAAA,
                                    49 => self.bg_color = DEFAULT_BG_COLOR,
                                    90 => self.fg_color = 0x555555,
                                    91 => self.fg_color = 0xFF5555,
                                    92 => self.fg_color = 0x55FF55,
                                    93 => self.fg_color = 0xFFFF55,
                                    94 => self.fg_color = 0x5599FF,
                                    95 => self.fg_color = 0xFF55FF,
                                    96 => self.fg_color = 0x55FFFF,
                                    97 => self.fg_color = 0xFFFFFF,
                                    100 => self.bg_color = 0x555555,
                                    101 => self.bg_color = 0xFF5555,
                                    102 => self.bg_color = 0x55FF55,
                                    103 => self.bg_color = 0xFFFF55,
                                    104 => self.bg_color = 0x5599FF,
                                    105 => self.bg_color = 0xFF55FF,
                                    106 => self.bg_color = 0x55FFFF,
                                    107 => self.bg_color = 0xFFFFFF,
                                    _ => {}
                                }
                            }
                        }
                        self.ansi_state = AnsiState::Normal;
                    }
                    'M' => {
                        // DL: Delete Line (удаление строк в области прокрутки)
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.delete_lines(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'L' => {
                        // IL: Insert Line (вставка пустых строк в области прокрутки)
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.insert_lines(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'P' => {
                        // DCH: Delete Character (удаление символов в строке)
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.delete_chars(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    '@' => {
                        // ICH: Insert Character (вставка пустых знакомест в строке)
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.insert_chars(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    'X' => {
                        // ECH: Erase Character (стирание символов пробелами без сдвига курсора)
                        let count = if self.ansi_param_count > 0 && self.ansi_params[0] > 0 {
                            self.ansi_params[0]
                        } else {
                            1
                        };
                        self.erase_chars(count);
                        self.ansi_state = AnsiState::Normal;
                    }
                    's' => {
                        // Сохранить позицию курсора (ANSI.SYS)
                        self.saved_col = self.col;
                        self.saved_row = self.row;
                        self.ansi_state = AnsiState::Normal;
                    }
                    'u' => {
                        // Восстановить позицию курсора (ANSI.SYS)
                        self.col = self.saved_col.min(self.cols.saturating_sub(1));
                        self.row = self.saved_row.min(self.rows.saturating_sub(1));
                        self.ansi_state = AnsiState::Normal;
                    }
                    _ => {
                        self.ansi_state = AnsiState::Normal;
                    }
                }
                return;
            }
        }

        match c {
            '\n' => {
                self.col = 0;
                self.advance_row();
            }
            '\r' => {
                self.col = 0;
            }
            '\t' => {
                let tab_stop = 8;
                let next_col = (self.col + tab_stop) & !(tab_stop - 1);
                let (fg, bg) = self.effective_colors();
                while self.col < next_col && self.col < self.cols {
                    super::font::draw_char(
                        &mut self.framebuffer,
                        MARGIN_X + self.col * super::font::FONT_WIDTH,
                        MARGIN_Y + self.row * super::font::FONT_HEIGHT,
                        ' ',
                        fg,
                        bg,
                    );
                    self.col += 1;
                }
            }
            '\x08' => {
                // В терминалах VT100 / Linux консоли ASCII BS (0x08) является строго недеструктивным
                // перемещением курсора влево. Очистка знакоместа пробелом приводила к тому,
                // что при любой навигации стрелками в nano/ncurses текст под курсором стирался.
                if self.col > 0 {
                    self.col -= 1;
                }
            }
            '\x00'..='\x1f' | '\x7f' => {
                // Игнорируем управляющие символы ASCII (NUL, BEL, SO, SI и т.д.),
                // чтобы они не выводились как REPLACEMENT_GLYPH (квадратики ▯)
            }
            c => {
                if self.col >= self.cols {
                    self.col = 0;
                    self.advance_row();
                }

                let (fg, bg) = self.effective_colors();
                super::font::draw_char(
                    &mut self.framebuffer,
                    MARGIN_X + self.col * super::font::FONT_WIDTH,
                    MARGIN_Y + self.row * super::font::FONT_HEIGHT,
                    c,
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

impl fmt::Write for DisplayConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.erase_cursor();
        for c in s.chars() {
            self.write_char_raw(c);
        }
        self.draw_cursor();
        Ok(())
    }
}

pub static CONSOLE: Mutex<Option<DisplayConsole>> = Mutex::new(None);

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

/// Проверяет, активирован ли прикладной режим стрелок терминала (DECCKM / smkx \x1b[?1h)
pub fn is_cursor_key_mode() -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        CONSOLE.lock().as_ref().map_or(false, |c| c.cursor_key_mode)
    })
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Глушим прерывания, чтобы не словить дедлок, если обработчик прерывания тоже вызовет print!
    interrupts::without_interrupts(|| {
        if let Some(mut guard) = CONSOLE.try_lock() {
            if let Some(console) = guard.as_mut() {
                let _ = console.write_fmt(args);
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
