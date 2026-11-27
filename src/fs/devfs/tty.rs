use core::sync::atomic::Ordering;
use x86_64::instructions::port::Port;

use super::types::CharDeviceOp;
use crate::fs::handle::{FileHandle, StdinHandle, StdoutStderrHandle};
use crate::fs::vfs::VfsError;

pub const TCGETS: u32 = 0x5401;
pub const TCSETS: u32 = 0x5402;
pub const TCSETSW: u32 = 0x5403;
pub const TCSETSF: u32 = 0x5404;
pub const TIOCSCTTY: u32 = 0x540E;
pub const TIOCGPGRP: u32 = 0x540F;
pub const TIOCSPGRP: u32 = 0x5410;
pub const TIOCGWINSZ: u32 = 0x5413;
pub const TIOCSWINSZ: u32 = 0x5414;
pub const FIONREAD: u32 = 0x541B;
pub const TIOCGPTN: u32 = 0x80045430;
pub const TIOCSPTLCK: u32 = 0x40045431;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleInputSource {
    Display,
    Serial,
}

static TTY_WINSIZE_OVERRIDE: spin::Mutex<Option<WinSize>> = spin::Mutex::new(None);
static SERIAL_WINSIZE: spin::Mutex<Option<WinSize>> = spin::Mutex::new(None);
static ACTIVE_INPUT_SOURCE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static LAST_SERIAL_PROBE_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SERIAL_PROBE_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_active_input_source(source: ConsoleInputSource) {
    let val = match source {
        ConsoleInputSource::Display => 0,
        ConsoleInputSource::Serial => 1,
    };
    ACTIVE_INPUT_SOURCE.store(val, Ordering::Release);
}

pub fn active_input_source() -> ConsoleInputSource {
    match ACTIVE_INPUT_SOURCE.load(Ordering::Acquire) {
        1 => ConsoleInputSource::Serial,
        _ => ConsoleInputSource::Display,
    }
}

pub fn is_serial_probe_active() -> bool {
    SERIAL_PROBE_ACTIVE.load(Ordering::Acquire)
}

struct SerialEscFilter {
    buf: [u8; 24],
    len: usize,
    start_tick: u64,
}

impl SerialEscFilter {
    const fn new() -> Self {
        Self {
            buf: [0; 24],
            len: 0,
            start_tick: 0,
        }
    }
}

static SERIAL_ESC_FILTER: spin::Mutex<SerialEscFilter> = spin::Mutex::new(SerialEscFilter::new());

fn forward_serial_byte(b: u8) {
    set_active_input_source(ConsoleInputSource::Serial);
    if crate::task::syscall::fs::handle_tty_isig_byte(b) {
        return;
    }
    let icrnl = crate::task::syscall::fs::is_icrnl_enabled();
    let byte_to_push = if b == b'\r' && icrnl { b'\n' } else { b };
    crate::fs::handle::push_stdin_byte(byte_to_push);
}

fn parse_xtwinops(buf: &[u8]) -> Option<(u16, u16)> {
    // Формат: ESC [ 8 ; <rows> ; <cols> t
    if !buf.starts_with(b"\x1b[8;") || !buf.ends_with(b"t") {
        return None;
    }
    let body = core::str::from_utf8(&buf[4..buf.len() - 1]).ok()?;
    let (rows_s, cols_s) = body.split_once(';')?;
    let rows: u16 = rows_s.parse().ok()?;
    let cols: u16 = cols_s.parse().ok()?;
    if rows >= 10 && cols >= 20 && rows < 1000 && cols < 1000 {
        Some((rows, cols))
    } else {
        None
    }
}

fn is_valid_serial_ctrl_prefix(buf: &[u8]) -> bool {
    match buf.len() {
        1 => buf[0] == 0x1b,
        2 => buf[1] == b'[',
        _ => {
            let last = buf[buf.len() - 1];
            last.is_ascii_digit() || last == b';' || last == b't' || last == b'R'
        }
    }
}

/// Фильтрует входящие байты COM1, перехватывая XTWINOPS (\x1b[8;rows;colst) и DSR (\x1b[rows;colsR)
/// для обновления размера окна и SIGWINCH.
pub fn push_serial_rx_byte(b: u8) {
    let mut flush_buf = [0u8; 24];
    let mut flush_len = 0usize;
    let mut new_winsize: Option<(u16, u16)> = None;

    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut f = SERIAL_ESC_FILTER.lock();
        if f.len == 0 {
            if b == 0x1b {
                f.buf[0] = b;
                f.len = 1;
                f.start_tick = crate::arch::x86_64::timer::ticks();
                return;
            }
            flush_buf[0] = b;
            flush_len = 1;
            return;
        }

        let idx = f.len;
        if idx < f.buf.len() {
            f.buf[idx] = b;
            f.len += 1;
        }

        let cur = &f.buf[..f.len];
        if b == b't' {
            if let Some((r, c)) = parse_xtwinops(cur) {
                new_winsize = Some((r, c));
                f.len = 0;
                return;
            }
        } else if b == b'R' {
            if let Some((r, c)) = parse_dsr_response(cur) {
                new_winsize = Some((r, c));
                f.len = 0;
                return;
            }
        }

        if !is_valid_serial_ctrl_prefix(cur) || f.len >= f.buf.len() {
            flush_len = f.len;
            flush_buf[..flush_len].copy_from_slice(cur);
            f.len = 0;
        }
    });

    if let Some((rows, cols)) = new_winsize {
        *SERIAL_WINSIZE.lock() = Some(WinSize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        });
        crate::task::signal::raise_foreground_signal(crate::task::syscall::signal::SIGWINCH);
        return;
    }

    for &byte in &flush_buf[..flush_len] {
        forward_serial_byte(byte);
    }
}

pub fn flush_stale_serial_esc() {
    let mut flush_buf = [0u8; 24];
    let mut flush_len = 0usize;
    let now = crate::arch::x86_64::timer::ticks();

    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut f = SERIAL_ESC_FILTER.lock();
        if f.len > 0 && now.saturating_sub(f.start_tick) >= 1 {
            flush_len = f.len;
            flush_buf[..flush_len].copy_from_slice(&f.buf[..flush_len]);
            f.len = 0;
        }
    });

    for &byte in &flush_buf[..flush_len] {
        forward_serial_byte(byte);
    }
}

fn parse_dsr_response(buf: &[u8]) -> Option<(u16, u16)> {
    // Ищем последовательность ESC [ <rows> ; <cols> R
    let esc_pos = buf.iter().position(|&b| b == 0x1b)?;
    let rest = &buf[esc_pos + 1..];
    if rest.first() != Some(&b'[') {
        return None;
    }
    let r_pos = rest.iter().position(|&b| b == b'R')?;
    let body = core::str::from_utf8(&rest[1..r_pos]).ok()?;
    let (rows_s, cols_s) = body.split_once(';')?;
    let rows: u16 = rows_s.parse().ok()?;
    let cols: u16 = cols_s.parse().ok()?;
    if rows >= 10 && cols >= 20 && rows < 1000 && cols < 1000 {
        Some((rows, cols))
    } else {
        None
    }
}

fn probe_serial_winsize() {
    if !crate::drivers::serial::is_present() {
        return;
    }
    let now = crate::arch::x86_64::timer::ticks();
    let last = LAST_SERIAL_PROBE_TICK.load(Ordering::Relaxed);
    // Раз в 5 секунд максимум: частые пробы были причиной мерцания хост-терминала
    if last != 0 && now.saturating_sub(last) < 5000 {
        return;
    }
    if crate::fs::handle::stdin_buffer_len() > 0 || crate::drivers::keyboard::key_queue_len() > 0 {
        return;
    }
    if SERIAL_PROBE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    LAST_SERIAL_PROBE_TICK.store(now, Ordering::Relaxed);

    // \x1b[18t — xterm "Report window size in chars": терминал отвечает \x1b[8;rows;colst
    // Не двигает курсор, поэтому не вызывает перерисовку хост-терминала.
    // Старый метод \x1b[s\x1b[999;999H\x1b[6n\x1b[u физически прыгал курсором в угол экрана
    // и обратно — именно это мерцало 5 раз в секунду в хост-терминале при вводе.
    crate::serial_print!("\x1b[18t");

    let mut resp = [0u8; 32];
    let mut len = 0usize;
    let deadline = now.saturating_add(20);

    while crate::arch::x86_64::timer::ticks() <= deadline {
        while let Some(b) = crate::drivers::serial::try_receive() {
            if len < resp.len() {
                resp[len] = b;
                len += 1;
            }
            if b == b't' {
                if let Some((rows, cols)) = parse_xtwinops(&resp[..len]) {
                    *SERIAL_WINSIZE.lock() = Some(WinSize {
                        ws_row: rows,
                        ws_col: cols,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    });
                    SERIAL_PROBE_ACTIVE.store(false, Ordering::Release);
                    return;
                }
            }
        }
        x86_64::instructions::interrupts::enable();
        x86_64::instructions::hlt();
    }

    // Если пришёл не-xtwinops ввод, не теряем его
    for &b in &resp[..len] {
        crate::fs::handle::push_stdin_byte(b);
    }
    SERIAL_PROBE_ACTIVE.store(false, Ordering::Release);
}

fn valid_winsize(ws: Option<WinSize>) -> Option<WinSize> {
    ws.filter(|w| w.ws_row > 0 && w.ws_col > 0)
}

fn display_winsize() -> Option<WinSize> {
    let guard = crate::drivers::display::CONSOLE.lock();
    let console = guard.as_ref()?;
    let r = console.rows as u16;
    let c = console.cols as u16;
    if r == 0 || c == 0 {
        return None;
    }
    Some(WinSize {
        ws_row: r,
        ws_col: c,
        ws_xpixel: c.saturating_mul(crate::drivers::font::FONT_WIDTH as u16),
        ws_ypixel: r.saturating_mul(crate::drivers::font::FONT_HEIGHT as u16),
    })
}

fn select_winsize(
    override_ws: Option<WinSize>,
    input_source: ConsoleInputSource,
    display_ws: Option<WinSize>,
    serial_ws: Option<WinSize>,
) -> WinSize {
    if let Some(ws) = valid_winsize(override_ws) {
        return ws;
    }
    // Выбираем размер того терминала, откуда пользователь реально вводит команды:
    // при вводе с COM1 ориентируемся на хост-терминал (SERIAL_WINSIZE),
    // при вводе с PS/2 — на окно QEMU (DisplayConsole), чтобы TUI не разъезжался.
    let (primary, fallback) = match input_source {
        ConsoleInputSource::Serial => (serial_ws, display_ws),
        ConsoleInputSource::Display => (display_ws, serial_ws),
    };
    valid_winsize(primary)
        .or_else(|| valid_winsize(fallback))
        .unwrap_or(WinSize {
            ws_row: 25,
            ws_col: 80,
            ws_xpixel: 640,
            ws_ypixel: 400,
        })
}

fn current_winsize() -> WinSize {
    let override_ws = *TTY_WINSIZE_OVERRIDE.lock();
    if let Some(ws) = valid_winsize(override_ws) {
        return ws;
    }
    let input_source = active_input_source();
    let display_ws = display_winsize();
    if input_source == ConsoleInputSource::Serial || display_ws.is_none() {
        probe_serial_winsize();
    }
    let serial_ws = *SERIAL_WINSIZE.lock();
    select_winsize(None, input_source, display_ws, serial_ws)
}

fn set_winsize(ws: WinSize) {
    if ws.ws_row == 0 || ws.ws_col == 0 {
        return;
    }
    *TTY_WINSIZE_OVERRIDE.lock() = Some(ws);
    crate::task::signal::raise_foreground_signal(crate::task::syscall::signal::SIGWINCH);
}

/// Обрабатывает типовые ioctl-запросы терминала (разрешение экрана, termios, группы процессов)
pub(crate) fn handle_terminal_ioctl(cmd: u32, arg: u64) -> Result<u64, i64> {
    if let Some(res) = crate::task::syscall::fs::handle_termios_ioctl(cmd as u64, arg) {
        return res;
    }
    match cmd {
        TIOCGWINSZ => {
            if !crate::task::syscall::validate_user_buffer(arg, 8) {
                return Err(-14);
            }
            let ws = current_winsize();
            unsafe {
                core::ptr::write_unaligned(arg as *mut WinSize, ws);
            }
            Ok(0)
        }
        TIOCSWINSZ => {
            if !crate::task::syscall::validate_user_buffer(arg, 8) {
                return Err(-14);
            }
            let ws = unsafe { core::ptr::read_unaligned(arg as *const WinSize) };
            set_winsize(ws);
            Ok(0)
        }
        TIOCGPGRP => {
            if !crate::task::syscall::validate_user_buffer(arg, 4) {
                return Err(-14);
            }
            let pgrp = crate::task::syscall::proc::sys_getpgrp() as u32;
            unsafe { core::ptr::write_unaligned(arg as *mut u32, pgrp); }
            Ok(0)
        }
        TIOCSPGRP | TIOCSCTTY => Ok(0),
        FIONREAD => {
            if !crate::task::syscall::validate_user_buffer(arg, 4) {
                return Err(-14);
            }
            let _ = crate::fs::handle::pump_stdin();
            let count = (crate::fs::handle::stdin_buffer_len() + crate::drivers::keyboard::key_queue_len()) as i32;
            unsafe { core::ptr::write_unaligned(arg as *mut i32, count); }
            Ok(0)
        }
        _ => Err(-25),
    }
}

/// /dev/tty: управляющий терминал текущего процесса
pub struct TtyDev;

impl CharDeviceOp for TtyDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        StdinHandle.read(buf).map_err(|_| VfsError::IoError)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        StdoutStderrHandle.write(buf).map_err(|_| VfsError::IoError)
    }

    fn poll_events(&self) -> u32 {
        StdinHandle.poll_events() | StdoutStderrHandle.poll_events()
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        handle_terminal_ioctl(cmd, arg)
    }
}

/// /dev/console и /dev/tty0: системная консоль ядра и активный виртуальный экран
pub struct ConsoleDev;

impl CharDeviceOp for ConsoleDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        StdinHandle.read(buf).map_err(|_| VfsError::IoError)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        StdoutStderrHandle.write(buf).map_err(|_| VfsError::IoError)
    }

    fn poll_events(&self) -> u32 {
        StdinHandle.poll_events() | StdoutStderrHandle.poll_events()
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        handle_terminal_ioctl(cmd, arg)
    }
}

/// /dev/tty1 - /dev/tty4: виртуальные текстовые консоли
pub struct VirtualConsoleDev {
    pub index: u8,
}

impl CharDeviceOp for VirtualConsoleDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        StdinHandle.read(buf).map_err(|_| VfsError::IoError)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        StdoutStderrHandle.write(buf).map_err(|_| VfsError::IoError)
    }

    fn poll_events(&self) -> u32 {
        StdinHandle.poll_events() | StdoutStderrHandle.poll_events()
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        handle_terminal_ioctl(cmd, arg)
    }
}

/// /dev/ttyS0, /dev/ttyS1: физические COM-порты UART 16550
pub struct TtySDev {
    pub base_port: u16,
}

impl CharDeviceOp for TtySDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let lsr_port = self.base_port + 5;
        let mut read_bytes = 0usize;

        while read_bytes < buf.len() {
            let lsr = unsafe { Port::<u8>::new(lsr_port).read() };
            if (lsr & 1) != 0 {
                let data = unsafe { Port::<u8>::new(self.base_port).read() };
                buf[read_bytes] = data;
                read_bytes += 1;
            } else {
                break;
            }
        }

        Ok(read_bytes)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let thr_port = self.base_port;
        let lsr_port = self.base_port + 5;

        for &b in buf {
            let mut timeout = 100_000;
            while timeout > 0 {
                let lsr = unsafe { Port::<u8>::new(lsr_port).read() };
                if (lsr & 0x20) != 0 {
                    break;
                }
                timeout -= 1;
            }
            unsafe {
                Port::<u8>::new(thr_port).write(b);
            }
        }
        Ok(buf.len())
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        handle_terminal_ioctl(cmd, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(rows: u16, cols: u16) -> WinSize {
        WinSize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    #[test_case]
    fn select_winsize_prefers_serial_when_serial_is_active() {
        let res = select_winsize(
            None,
            ConsoleInputSource::Serial,
            Some(ws(48, 128)),
            Some(ws(55, 190)),
        );
        assert_eq!(res.ws_row, 55);
        assert_eq!(res.ws_col, 190);
    }

    #[test_case]
    fn select_winsize_prefers_display_when_display_is_active() {
        let res = select_winsize(
            None,
            ConsoleInputSource::Display,
            Some(ws(48, 128)),
            Some(ws(55, 190)),
        );
        assert_eq!(res.ws_row, 48);
        assert_eq!(res.ws_col, 128);
    }

    #[test_case]
    fn select_winsize_respects_override_and_falls_back_to_default() {
        let over = select_winsize(
            Some(ws(30, 100)),
            ConsoleInputSource::Serial,
            Some(ws(48, 128)),
            Some(ws(55, 190)),
        );
        assert_eq!(over.ws_row, 30);
        assert_eq!(over.ws_col, 100);

        let fallback = select_winsize(None, ConsoleInputSource::Serial, None, None);
        assert_eq!(fallback.ws_row, 25);
        assert_eq!(fallback.ws_col, 80);
    }

    #[test_case]
    fn parse_serial_winsize_responses() {
        assert_eq!(parse_dsr_response(b"\x1b[50;180R"), Some((50, 180)));
        assert_eq!(parse_xtwinops(b"\x1b[8;50;180t"), Some((50, 180)));
        assert!(is_valid_serial_ctrl_prefix(b"\x1b[50;180R"));
        assert!(is_valid_serial_ctrl_prefix(b"\x1b[8;50;180t"));
    }
}
