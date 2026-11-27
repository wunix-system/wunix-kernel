pub mod vfs;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use wunix_drv::abi::fuse::*;

use crate::fs::handle::{FileHandle, EPOLLIN, EPOLLOUT};
use crate::fs::vfs::{Inode, InodeType, Metadata, VfsError};

pub use vfs::{FuseFileSystem, FuseInode};

const FUSE_TIMEOUT_TICKS: u64 = 500; // 5 секунд при 100 Гц

struct FusePendingCall {
    unique: u64,
    done: AtomicBool,
    response: Mutex<Option<Vec<u8>>>,
}

/// Очередь обмена между VFS ядра и user-space демоном через `/dev/fuse`.
pub struct FuseSession {
    next_unique: AtomicU64,
    dead: AtomicBool,
    pub(crate) max_write: AtomicU32,
    requests: Mutex<VecDeque<Vec<u8>>>,
    waiters: Mutex<Vec<Arc<FusePendingCall>>>,
}

impl FuseSession {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_unique: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            max_write: AtomicU32::new(65536),
            requests: Mutex::new(VecDeque::new()),
            waiters: Mutex::new(Vec::new()),
        })
    }

    pub fn shutdown(&self) {
        self.dead.store(true, Ordering::Release);
        let waiters = self.waiters.lock();
        for w in waiters.iter() {
            w.done.store(true, Ordering::Release);
        }
    }

    /// Синхронно отправляет FUSE-запрос в user space и ожидает ответа.
    pub fn call(&self, opcode: u32, nodeid: u64, payload: &[u8]) -> Result<Vec<u8>, VfsError> {
        if self.dead.load(Ordering::Acquire) {
            return Err(VfsError::IoError);
        }

        let unique = self.next_unique.fetch_add(1, Ordering::Relaxed);
        let total_len = (core::mem::size_of::<FuseInHeader>() + payload.len()) as u32;
        let hdr = FuseInHeader {
            len: total_len,
            opcode,
            unique,
            nodeid,
            uid: crate::task::current_thread_euid(),
            gid: crate::task::current_thread_egid(),
            pid: crate::task::current_thread_tgid() as u32,
            padding: 0,
        };

        let mut msg = Vec::with_capacity(total_len as usize);
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(
                &hdr as *const FuseInHeader as *const u8,
                core::mem::size_of::<FuseInHeader>(),
            )
        };
        msg.extend_from_slice(hdr_bytes);
        msg.extend_from_slice(payload);

        let waiter = Arc::new(FusePendingCall {
            unique,
            done: AtomicBool::new(false),
            response: Mutex::new(None),
        });

        self.waiters.lock().push(waiter.clone());
        self.requests.lock().push_back(msg);

        let start = crate::arch::x86_64::timer::ticks();
        while !waiter.done.load(Ordering::Acquire) {
            if self.dead.load(Ordering::Acquire) {
                self.remove_waiter(unique);
                return Err(VfsError::IoError);
            }
            if crate::arch::x86_64::timer::ticks().saturating_sub(start) > FUSE_TIMEOUT_TICKS {
                self.remove_waiter(unique);
                return Err(VfsError::IoError);
            }
            crate::task::scheduler::yield_now();
        }

        self.remove_waiter(unique);
        let resp = waiter.response.lock().take().ok_or(VfsError::IoError)?;
        if resp.len() < core::mem::size_of::<FuseOutHeader>() {
            return Err(VfsError::IoError);
        }

        let out_hdr = unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseOutHeader) };
        if out_hdr.error != 0 {
            return Err(errno_to_vfs(-out_hdr.error));
        }

        Ok(resp[core::mem::size_of::<FuseOutHeader>()..].to_vec())
    }

    fn remove_waiter(&self, unique: u64) {
        let mut waiters = self.waiters.lock();
        if let Some(pos) = waiters.iter().position(|w| w.unique == unique) {
            waiters.swap_remove(pos);
        }
    }

    pub fn pop_request(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let mut q = self.requests.lock();
        let Some(req) = q.front() else {
            return Err(-11);
        };
        if buf.len() < req.len() {
            return Err(-22);
        }
        let req = q.pop_front().unwrap();
        buf[..req.len()].copy_from_slice(&req);
        Ok(req.len())
    }

    pub fn push_response(&self, data: &[u8]) -> Result<usize, i64> {
        if data.len() < core::mem::size_of::<FuseOutHeader>() {
            return Err(-22);
        }
        let hdr = unsafe { core::ptr::read_unaligned(data.as_ptr() as *const FuseOutHeader) };
        let waiters = self.waiters.lock();
        if let Some(w) = waiters.iter().find(|w| w.unique == hdr.unique) {
            *w.response.lock() = Some(data.to_vec());
            w.done.store(true, Ordering::Release);
            Ok(data.len())
        } else {
            Err(-2)
        }
    }

    pub fn has_pending_requests(&self) -> bool {
        !self.requests.lock().is_empty()
    }
}

fn errno_to_vfs(err: i32) -> VfsError {
    match err {
        2 => VfsError::NotFound,
        13 | 1 => VfsError::PermissionDenied,
        17 => VfsError::AlreadyExists,
        20 => VfsError::NotADirectory,
        21 => VfsError::IsADirectory,
        22 => VfsError::InvalidPath,
        38 | 95 => VfsError::NotSupported,
        _ => VfsError::IoError,
    }
}

/// Открытый дескриптор `/dev/fuse`, принадлежащий user-space демону.
pub struct FuseDevHandle {
    session: Arc<FuseSession>,
    flags: AtomicU32,
}

impl FuseDevHandle {
    pub fn new(flags: u32) -> Self {
        Self {
            session: FuseSession::new(),
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for FuseDevHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        loop {
            match self.session.pop_request(buf) {
                Ok(n) => return Ok(n),
                Err(-11) => {
                    if (self.flags.load(Ordering::Relaxed) & crate::fs::O_NONBLOCK) != 0 {
                        return Err(-11);
                    }
                    if self.session.dead.load(Ordering::Acquire) {
                        return Ok(0);
                    }
                    crate::task::scheduler::yield_now();
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        self.session.push_response(buf)
    }

    fn poll_events(&self) -> u32 {
        let mut ev = EPOLLOUT;
        if self.session.has_pending_requests() {
            ev |= EPOLLIN;
        }
        ev
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn close(&self) {
        self.session.shutdown();
    }

    fn as_fuse_session(&self) -> Option<Arc<FuseSession>> {
        Some(self.session.clone())
    }
}

/// Узел `/dev/fuse` в DevFS — каждое открытие порождает независимую FUSE-сессию.
pub struct FuseDevInode;

impl Inode for FuseDevInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: InodeType::CharDevice,
            size: 0,
            inode_id: 0xF05E_0001,
            permissions: 0o666,
            uid: 0,
            gid: 0,
            rdev: crate::fs::devfs::makedev(crate::fs::devfs::MISC_MAJOR, crate::fs::devfs::FUSE_MINOR),
        }
    }

    fn open_handle(&self, flags: u32, _path: Option<alloc::string::String>) -> Option<Arc<dyn FileHandle>> {
        Some(Arc::new(FuseDevHandle::new(flags)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_fuse_session_init_handshake_in_kernel() {
        let session = FuseSession::new();
        assert!(!session.dead.load(Ordering::Acquire));

        let mut req_buf = [0u8; 64];
        let n = session.pop_request(&mut req_buf);
        assert_eq!(n, Err(-11));
    }
}
