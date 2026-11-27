use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use spin::{Mutex, RwLock};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::fs::vfs::{Inode, InodeType, Metadata, VfsError};
use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

static NEXT_SHM_INODE_ID: AtomicU64 = AtomicU64::new(0x5A00_0001);

/// Владелец общих физических страниц памяти (`/dev/shm/*`, `memfd_create`).
/// Хранится в `Arc`, поэтому страницы освобождаются только после закрытия всех fd и VMA.
pub struct ShmBacking {
    pub frames: Mutex<Vec<PhysFrame<Size4KiB>>>,
}

impl ShmBacking {
    pub fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
        }
    }

    pub fn ensure_pages(&self, page_count: usize) -> Result<(), VfsError> {
        let mut frames = self.frames.lock();
        let hhdm = hhdm_offset();
        while frames.len() < page_count {
            let frame = allocate_frame().ok_or(VfsError::IoError)?;
            let ptr = (hhdm + frame.start_address().as_u64()) as *mut u8;
            unsafe {
                core::ptr::write_bytes(ptr, 0, 4096);
            }
            frames.push(frame);
        }
        Ok(())
    }

    pub fn shrink_pages(&self, page_count: usize) {
        let mut frames = self.frames.lock();
        while frames.len() > page_count {
            if let Some(frame) = frames.pop() {
                deallocate_frame(frame);
            }
        }
    }
}

impl Drop for ShmBacking {
    fn drop(&mut self) {
        let frames = self.frames.get_mut();
        for frame in frames.drain(..) {
            deallocate_frame(frame);
        }
    }
}

/// Разделяемый файл в `/dev/shm` или `memfd_create` с zero-copy `MAP_SHARED` отображением.
pub struct ShmInode {
    inode_id: u64,
    size: RwLock<usize>,
    permissions: AtomicU16,
    uid: AtomicU32,
    gid: AtomicU32,
    backing: Arc<ShmBacking>,
}

impl ShmInode {
    pub fn new() -> Self {
        Self {
            inode_id: NEXT_SHM_INODE_ID.fetch_add(1, Ordering::Relaxed),
            size: RwLock::new(0),
            permissions: AtomicU16::new(0o666),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            backing: Arc::new(ShmBacking::new()),
        }
    }
}

impl Inode for ShmInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: InodeType::File,
            size: *self.size.read(),
            inode_id: self.inode_id,
            permissions: self.permissions.load(Ordering::Relaxed),
            uid: self.uid.load(Ordering::Relaxed),
            gid: self.gid.load(Ordering::Relaxed),
            rdev: 0,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let total = *self.size.read();
        if offset >= total || buf.is_empty() {
            return Ok(0);
        }
        let to_read = buf.len().min(total - offset);
        let frames = self.backing.frames.lock();
        let hhdm = hhdm_offset();
        let mut copied = 0;
        while copied < to_read {
            let pos = offset + copied;
            let page_idx = pos / 4096;
            let page_off = pos % 4096;
            let chunk = (4096 - page_off).min(to_read - copied);
            if let Some(frame) = frames.get(page_idx) {
                let src = (hhdm + frame.start_address().as_u64() + page_off as u64) as *const u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr().add(copied), chunk);
                }
            } else {
                buf[copied..copied + chunk].fill(0);
            }
            copied += chunk;
        }
        Ok(copied)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(buf.len()).ok_or(VfsError::InvalidPath)?;
        let pages = (end + 4095) / 4096;
        self.backing.ensure_pages(pages)?;

        let frames = self.backing.frames.lock();
        let hhdm = hhdm_offset();
        let mut written = 0;
        while written < buf.len() {
            let pos = offset + written;
            let page_idx = pos / 4096;
            let page_off = pos % 4096;
            let chunk = (4096 - page_off).min(buf.len() - written);
            let frame = frames.get(page_idx).ok_or(VfsError::IoError)?;
            let dst = (hhdm + frame.start_address().as_u64() + page_off as u64) as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(written), dst, chunk);
            }
            written += chunk;
        }
        drop(frames);

        let mut sz = self.size.write();
        if end > *sz {
            *sz = end;
        }
        Ok(written)
    }

    fn truncate(&self, len: usize) -> Result<(), VfsError> {
        let pages = (len + 4095) / 4096;
        let cur = *self.size.read();
        if len > cur {
            self.backing.ensure_pages(pages)?;
        } else if len < cur {
            self.backing.shrink_pages(pages);
        }
        *self.size.write() = len;
        Ok(())
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        self.permissions.store(mode, Ordering::Relaxed);
        Ok(())
    }

    fn chown(&self, uid: u32, gid: u32) -> Result<(), VfsError> {
        self.uid.store(uid, Ordering::Relaxed);
        self.gid.store(gid, Ordering::Relaxed);
        Ok(())
    }

    fn mmap_backing(&self, offset: usize, len: usize) -> Option<crate::fs::vfs::MmapBacking> {
        if offset != 0 || len == 0 {
            return None;
        }
        let pages = (len + 4095) / 4096;
        if self.backing.ensure_pages(pages).is_err() {
            return None;
        }
        let mut sz = self.size.write();
        if len > *sz {
            *sz = len;
        }
        Some(crate::fs::vfs::MmapBacking::Shared(self.backing.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_shm_inode_read_write_and_shared_backing() {
        let inode = ShmInode::new();
        let n = inode.write_at(0, b"shm-zero-copy").unwrap();
        assert_eq!(n, 13);
        let mut out = [0u8; 13];
        let rn = inode.read_at(0, &mut out).unwrap();
        assert_eq!(rn, 13);
        assert_eq!(&out, b"shm-zero-copy");

        let backing = inode.mmap_backing(0, 4096);
        assert!(backing.is_some());
    }
}
