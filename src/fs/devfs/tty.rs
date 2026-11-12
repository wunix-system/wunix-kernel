use core::sync::atomic::{AtomicU32, Ordering};
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

fn get_display_winsize() -> WinSize {
    let (rows, cols) = if let Some(console) = crate::drivers::display::CONSOLE.lock().as_ref() {
        (console.rows as u16, console.cols as u16)
    } else {
        (25, 80)
    };
    WinSize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Обрабатывает типовые ioctl-запросы терминала (разрешение экрана, termios, группы процессов)
fn handle_terminal_ioctl(cmd: u32, arg: u64) -> Result<u64, i64> {
    if let Some(res) = crate::task::syscall::fs::handle_termios_ioctl(cmd as u64, arg) {
        return res;
    }
    match cmd {
        TIOCGWINSZ => {
            if !crate::task::syscall::validate_user_buffer(arg, 8) {
                return Err(-14);
            }
            let ws = get_display_winsize();
            unsafe {
                core::ptr::write_unaligned(arg as *mut WinSize, ws);
            }
            Ok(0)
        }
        TIOCSWINSZ => Ok(0),
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
        match cmd {
            TIOCGWINSZ => {
                if !crate::task::syscall::validate_user_buffer(arg, 8) {
                    return Err(-14);
                }
                let ws = WinSize {
                    ws_row: 25,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                unsafe { core::ptr::write_unaligned(arg as *mut WinSize, ws); }
                Ok(0)
            }
            TCGETS | TCSETS | TCSETSW | TCSETSF => Ok(0),
            _ => Err(-25),
        }
    }
}

static NEXT_PTS_ID: AtomicU32 = AtomicU32::new(0);

/// /dev/ptmx: мультиплексор псевдотерминалов (POSIX Pseudo-Terminal Master Multiplexer)
pub struct PtmxDev;

impl CharDeviceOp for PtmxDev {
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Ok(0)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        match cmd {
            TIOCGPTN => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let pts_id = NEXT_PTS_ID.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    core::ptr::write_unaligned(arg as *mut u32, pts_id);
                }
                Ok(0)
            }
            TIOCSPTLCK => Ok(0),
            _ => Err(-25),
        }
    }
}
