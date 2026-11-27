use alloc::collections::VecDeque;
use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use super::tty::{WinSize, TIOCGPTN, TIOCGWINSZ, TIOCSPTLCK, TIOCSWINSZ};
use super::types::{CharDeviceOp, TTY_MAJOR};
use crate::fs::handle::{FileHandle, EPOLLIN, EPOLLOUT};
use crate::fs::vfs::{Inode, InodeType, Metadata, VfsError};

static NEXT_PTS_ID: AtomicU32 = AtomicU32::new(0);

/// Общее состояние пары псевдотерминала (master `/dev/ptmx` <-> slave `/dev/pts/N`).
pub struct PtyPair {
    pub id: u32,
    pub locked: AtomicBool,
    pub m2s: Mutex<VecDeque<u8>>,
    pub s2m: Mutex<VecDeque<u8>>,
    pub winsize: Mutex<WinSize>,
}

impl PtyPair {
    pub fn new(id: u32) -> Arc<Self> {
        Arc::new(Self {
            id,
            locked: AtomicBool::new(true),
            m2s: Mutex::new(VecDeque::new()),
            s2m: Mutex::new(VecDeque::new()),
            winsize: Mutex::new(WinSize {
                ws_row: 25,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
        })
    }
}

/// Дескриптор master-стороны PTY, возвращаемый при каждом `open("/dev/ptmx")`.
pub struct PtyMasterHandle {
    pair: Arc<PtyPair>,
    flags: AtomicU32,
}

impl PtyMasterHandle {
    pub fn new(pair: Arc<PtyPair>, flags: u32) -> Self {
        Self {
            pair,
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for PtyMasterHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let mut q = self.pair.s2m.lock();
        if q.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(q.len());
        for dst in buf[..n].iter_mut() {
            *dst = q.pop_front().unwrap();
        }
        Ok(n)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        let mut q = self.pair.m2s.lock();
        for &b in buf {
            q.push_back(b);
        }
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let mut ev = EPOLLOUT;
        if !self.pair.s2m.lock().is_empty() {
            ev |= EPOLLIN;
        }
        ev
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        match cmd {
            TIOCGPTN => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                unsafe {
                    core::ptr::write_unaligned(arg as *mut u32, self.pair.id);
                }
                Ok(0)
            }
            TIOCSPTLCK => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let val = unsafe { core::ptr::read_unaligned(arg as *const i32) };
                self.pair.locked.store(val != 0, Ordering::Release);
                Ok(0)
            }
            TIOCGWINSZ => {
                let sz = core::mem::size_of::<WinSize>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ws = *self.pair.winsize.lock();
                unsafe {
                    core::ptr::write_unaligned(arg as *mut WinSize, ws);
                }
                Ok(0)
            }
            TIOCSWINSZ => {
                let sz = core::mem::size_of::<WinSize>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ws = unsafe { core::ptr::read_unaligned(arg as *const WinSize) };
                *self.pair.winsize.lock() = ws;
                Ok(0)
            }
            _ => super::tty::handle_terminal_ioctl(cmd, arg),
        }
    }
}

/// Узел slave-стороны PTY (`/dev/pts/N`).
pub struct PtySlaveDev {
    pair: Arc<PtyPair>,
}

impl CharDeviceOp for PtySlaveDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.pair.locked.load(Ordering::Acquire) {
            return Err(VfsError::PermissionDenied);
        }
        let mut q = self.pair.m2s.lock();
        if q.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(q.len());
        for dst in buf[..n].iter_mut() {
            *dst = q.pop_front().unwrap();
        }
        Ok(n)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if self.pair.locked.load(Ordering::Acquire) {
            return Err(VfsError::PermissionDenied);
        }
        let mut q = self.pair.s2m.lock();
        for &b in buf {
            q.push_back(b);
        }
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let mut ev = EPOLLOUT;
        if !self.pair.m2s.lock().is_empty() {
            ev |= EPOLLIN;
        }
        ev
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        match cmd {
            TIOCGWINSZ => {
                let sz = core::mem::size_of::<WinSize>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ws = *self.pair.winsize.lock();
                unsafe {
                    core::ptr::write_unaligned(arg as *mut WinSize, ws);
                }
                Ok(0)
            }
            TIOCSWINSZ => {
                let sz = core::mem::size_of::<WinSize>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ws = unsafe { core::ptr::read_unaligned(arg as *const WinSize) };
                *self.pair.winsize.lock() = ws;
                Ok(0)
            }
            _ => super::tty::handle_terminal_ioctl(cmd, arg),
        }
    }
}

/// Узел `/dev/ptmx` в DevFS — каждое открытие выделяет новую пару `/dev/pts/N`.
pub struct PtmxInode;

impl Inode for PtmxInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: InodeType::CharDevice,
            size: 0,
            inode_id: 0x5054_4D58,
            permissions: 0o666,
            uid: 0,
            gid: 0,
            rdev: super::types::makedev(super::types::TTYAUX_MAJOR, 2),
        }
    }

    fn open_handle(&self, flags: u32, _path: Option<alloc::string::String>) -> Option<Arc<dyn FileHandle>> {
        let id = NEXT_PTS_ID.fetch_add(1, Ordering::Relaxed);
        let pair = PtyPair::new(id);
        if let Some(devfs) = super::get_devfs() {
            let pts_path = format!("pts/{}", id);
            devfs.register_char(
                &pts_path,
                TTY_MAJOR,
                id,
                Arc::new(PtySlaveDev { pair: pair.clone() }),
                0o620,
            );
        }
        Some(Arc::new(PtyMasterHandle::new(pair, flags)))
    }
}

/// Узел `/dev/hvc0` поверх `VirtioConsole` для паравиртуального обмена с хостом.
pub struct HvcConsoleDev {
    index: usize,
}

impl HvcConsoleDev {
    pub fn new(index: usize) -> Self {
        Self { index }
    }
}

impl CharDeviceOp for HvcConsoleDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let dev = crate::drivers::manager::get_console_device(self.index).ok_or(VfsError::NotFound)?;
        match dev.lock().read(buf) {
            Ok(Some(n)) => Ok(n),
            Ok(None) => Ok(0),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let dev = crate::drivers::manager::get_console_device(self.index).ok_or(VfsError::NotFound)?;
        dev.lock().write(buf).map_err(|_| VfsError::IoError)
    }

    fn is_tty(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pty_pair_master_slave_roundtrip() {
        let pair = PtyPair::new(0);
        let master = PtyMasterHandle::new(pair.clone(), 0);
        let slave = PtySlaveDev { pair: pair.clone() };
        assert_eq!(master.write(b"hi\n"), Ok(3));
        let mut out = [0u8; 8];
        // Пока PTY заблокирован (unlockpt не вызван), slave запрещён для чтения.
        assert_eq!(slave.read(0, &mut out), Err(VfsError::PermissionDenied));
        pair.locked.store(false, Ordering::Release);
        assert_eq!(slave.read(0, &mut out), Ok(3));
        assert_eq!(&out[..3], b"hi\n");
        assert_eq!(slave.write(0, b"ok"), Ok(2));
        assert_eq!(master.read(&mut out), Ok(2));
        assert_eq!(&out[..2], b"ok");
    }
}
