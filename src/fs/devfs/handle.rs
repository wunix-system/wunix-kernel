use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use crate::fs::handle::FileHandle;
use crate::fs::vfs::Inode;

/// Дескриптор открытого файла устройства в файловой таблице процесса.
/// Обеспечивает прямую трансляцию POSIX-вызовов read/write/ioctl в соответствующий узел Inode.
pub struct DevFileHandle {
    pub inode: Arc<dyn Inode>,
    pub offset: Mutex<usize>,
    pub flags: AtomicU32,
    pub path: Option<String>,
}

impl DevFileHandle {
    pub fn new(inode: Arc<dyn Inode>, flags: u32, path: Option<String>) -> Self {
        Self {
            inode,
            offset: Mutex::new(0),
            flags: AtomicU32::new(flags),
            path,
        }
    }
}

impl FileHandle for DevFileHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let mut off = self.offset.lock();
        let bytes = self.inode.read_at(*off, buf).map_err(|_| -5i64)?;
        *off += bytes;
        Ok(bytes)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        let mut off = self.offset.lock();
        let bytes = self.inode.write_at(*off, buf).map_err(|_| -5i64)?;
        *off += bytes;
        Ok(bytes)
    }

    fn poll_events(&self) -> u32 {
        if self.is_tty() {
            crate::fs::handle::StdinHandle.poll_events() | crate::fs::handle::StdoutStderrHandle.poll_events()
        } else {
            crate::fs::EPOLLIN | crate::fs::EPOLLOUT
        }
    }

    fn is_tty(&self) -> bool {
        let meta = self.inode.metadata();
        if meta.itype == crate::fs::InodeType::CharDevice {
            let maj = super::types::major(meta.rdev);
            maj == 4 || maj == 5
        } else {
            false
        }
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn as_inode(&self) -> Option<Arc<dyn Inode>> {
        Some(self.inode.clone())
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        self.inode.ioctl(cmd, arg)
    }

    fn offset(&self) -> usize {
        *self.offset.lock()
    }

    fn set_offset(&self, offset: usize) {
        *self.offset.lock() = offset;
    }
}
