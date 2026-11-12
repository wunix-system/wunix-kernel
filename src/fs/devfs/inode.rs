use alloc::sync::Arc;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::types::CharDeviceOp;
use crate::drivers::BlockDevice;
use crate::fs::vfs::{Inode, InodeType, Metadata, VfsError};

static NEXT_DEV_INODE_ID: AtomicU64 = AtomicU64::new(0xDE00_0001);

fn next_inode_id() -> u64 {
    NEXT_DEV_INODE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Индексный узел виртуального устройства (/dev/*).
/// Унифицирует символьные драйверы (CharDeviceOp) и блочные диски (BlockDevice)
/// в единую модель VFS без заглушек и строковых сопоставлений.
pub struct DevInode {
    pub(crate) inode_id: u64,
    pub(crate) itype: InodeType,
    pub(crate) rdev: u64,
    pub(crate) permissions: AtomicU16,
    pub(crate) uid: AtomicU32,
    pub(crate) gid: AtomicU32,
    pub(crate) op: Option<Arc<dyn CharDeviceOp>>,
    pub(crate) block_dev: Option<Arc<Mutex<dyn BlockDevice>>>,
}

impl DevInode {
    pub fn new_char(rdev: u64, permissions: u16, op: Arc<dyn CharDeviceOp>) -> Self {
        Self {
            inode_id: next_inode_id(),
            itype: InodeType::CharDevice,
            rdev,
            permissions: AtomicU16::new(permissions),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            op: Some(op),
            block_dev: None,
        }
    }

    pub fn new_block(rdev: u64, permissions: u16, block_dev: Arc<Mutex<dyn BlockDevice>>) -> Self {
        Self {
            inode_id: next_inode_id(),
            itype: InodeType::BlockDevice,
            rdev,
            permissions: AtomicU16::new(permissions),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            op: None,
            block_dev: Some(block_dev),
        }
    }

    pub fn rdev(&self) -> u64 {
        self.rdev
    }

    pub fn char_op(&self) -> Option<&Arc<dyn CharDeviceOp>> {
        self.op.as_ref()
    }

    pub fn block_dev(&self) -> Option<&Arc<Mutex<dyn BlockDevice>>> {
        self.block_dev.as_ref()
    }
}

impl Inode for DevInode {
    fn metadata(&self) -> Metadata {
        let size = if let Some(op) = &self.op {
            op.size()
        } else if let Some(blk) = &self.block_dev {
            let dev = blk.lock();
            (dev.sector_count() * dev.sector_size() as u64) as usize
        } else {
            0
        };

        Metadata {
            itype: self.itype,
            size,
            inode_id: self.inode_id,
            permissions: self.permissions.load(Ordering::Relaxed),
            uid: self.uid.load(Ordering::Relaxed),
            gid: self.gid.load(Ordering::Relaxed),
            rdev: self.rdev,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        if let Some(op) = &self.op {
            return op.read(offset, buf);
        }

        if let Some(blk) = &self.block_dev {
            let mut dev = blk.lock();
            let sector_size = dev.sector_size();
            let total_bytes = (dev.sector_count() * sector_size as u64) as usize;

            if offset >= total_bytes {
                return Ok(0);
            }

            let to_read = buf.len().min(total_bytes - offset);
            let mut bytes_copied = 0usize;
            let mut scratch = [0u8; 512];

            while bytes_copied < to_read {
                let curr_offset = offset + bytes_copied;
                let sector = (curr_offset / sector_size) as u64;
                let offset_in_sec = curr_offset % sector_size;
                let chunk = (sector_size - offset_in_sec).min(to_read - bytes_copied);

                dev.read_sectors(sector, &mut scratch).map_err(|_| VfsError::IoError)?;
                buf[bytes_copied..bytes_copied + chunk]
                    .copy_from_slice(&scratch[offset_in_sec..offset_in_sec + chunk]);

                bytes_copied += chunk;
            }

            return Ok(bytes_copied);
        }

        Err(VfsError::NotSupported)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        if let Some(op) = &self.op {
            return op.write(offset, buf);
        }

        if let Some(blk) = &self.block_dev {
            let mut dev = blk.lock();
            let sector_size = dev.sector_size();
            let total_bytes = (dev.sector_count() * sector_size as u64) as usize;

            if offset >= total_bytes {
                return Err(VfsError::IoError);
            }

            let to_write = buf.len().min(total_bytes - offset);
            let mut bytes_written = 0usize;
            let mut scratch = [0u8; 512];

            while bytes_written < to_write {
                let curr_offset = offset + bytes_written;
                let sector = (curr_offset / sector_size) as u64;
                let offset_in_sec = curr_offset % sector_size;
                let chunk = (sector_size - offset_in_sec).min(to_write - bytes_written);

                if offset_in_sec != 0 || chunk < sector_size {
                    dev.read_sectors(sector, &mut scratch).map_err(|_| VfsError::IoError)?;
                    scratch[offset_in_sec..offset_in_sec + chunk]
                        .copy_from_slice(&buf[bytes_written..bytes_written + chunk]);
                    dev.write_sectors(sector, &scratch).map_err(|_| VfsError::IoError)?;
                } else {
                    dev.write_sectors(sector, &buf[bytes_written..bytes_written + sector_size])
                        .map_err(|_| VfsError::IoError)?;
                }

                bytes_written += chunk;
            }

            return Ok(bytes_written);
        }

        Err(VfsError::NotSupported)
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

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        if let Some(op) = &self.op {
            return op.ioctl(cmd, arg);
        }

        if let Some(blk) = &self.block_dev {
            let dev = blk.lock();
            return match cmd {
                0x1260 => {
                    let sectors = dev.sector_count();
                    if !crate::task::syscall::validate_user_buffer(arg, 8) {
                        return Err(-14);
                    }
                    unsafe { core::ptr::write_unaligned(arg as *mut u64, sectors); }
                    Ok(0)
                }
                0x1268 => {
                    let ssz = dev.sector_size() as u32;
                    if !crate::task::syscall::validate_user_buffer(arg, 4) {
                        return Err(-14);
                    }
                    unsafe { core::ptr::write_unaligned(arg as *mut u32, ssz); }
                    Ok(0)
                }
                0x80081272 => {
                    let bytes = dev.sector_count() * dev.sector_size() as u64;
                    if !crate::task::syscall::validate_user_buffer(arg, 8) {
                        return Err(-14);
                    }
                    unsafe { core::ptr::write_unaligned(arg as *mut u64, bytes); }
                    Ok(0)
                }
                _ => Err(-25),
            };
        }

        Err(-25)
    }
}
