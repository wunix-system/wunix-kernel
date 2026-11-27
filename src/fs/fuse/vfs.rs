use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use wunix_drv::abi::fuse::*;

use super::FuseSession;
use crate::fs::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};

/// Файловая система VFS поверх user-space FUSE-демона.
pub struct FuseFileSystem {
    session: Arc<FuseSession>,
    root: Arc<FuseInode>,
}

impl FuseFileSystem {
    pub fn new(session: Arc<FuseSession>) -> Self {
        let init_in = FuseInitIn {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 65536,
            flags: 0,
        };
        let init_bytes = unsafe {
            core::slice::from_raw_parts(
                &init_in as *const FuseInitIn as *const u8,
                core::mem::size_of::<FuseInitIn>(),
            )
        };
        if let Ok(resp) = session.call(FUSE_INIT, 0, init_bytes) {
            if resp.len() >= core::mem::size_of::<FuseInitOut>() {
                let init_out =
                    unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseInitOut) };
                if init_out.max_write > 0 {
                    session.max_write.store(init_out.max_write, Ordering::Release);
                }
            }
        }

        let root = Arc::new(FuseInode {
            nodeid: FUSE_ROOT_ID,
            itype: InodeType::Directory,
            session: session.clone(),
        });
        Self { session, root }
    }
}

impl Drop for FuseFileSystem {
    fn drop(&mut self) {
        let _ = self.session.call(FUSE_DESTROY, 0, &[]);
        self.session.shutdown();
    }
}

impl FileSystem for FuseFileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn name(&self) -> &'static str {
        "fuse"
    }
}

pub struct FuseInode {
    nodeid: u64,
    itype: InodeType,
    session: Arc<FuseSession>,
}

fn attr_to_itype(mode: u32) -> InodeType {
    match mode & 0o170000 {
        0o040000 => InodeType::Directory,
        0o120000 => InodeType::Symlink,
        0o020000 => InodeType::CharDevice,
        0o060000 => InodeType::BlockDevice,
        _ => InodeType::File,
    }
}

fn dirent_type_to_itype(typ: u32) -> InodeType {
    match typ {
        4 => InodeType::Directory,
        10 => InodeType::Symlink,
        2 => InodeType::CharDevice,
        6 => InodeType::BlockDevice,
        _ => InodeType::File,
    }
}

fn as_bytes<T>(val: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(val as *const T as *const u8, core::mem::size_of::<T>()) }
}

impl Inode for FuseInode {
    fn metadata(&self) -> Metadata {
        let req = FuseGetattrIn::default();
        if let Ok(resp) = self.session.call(FUSE_GETATTR, self.nodeid, as_bytes(&req)) {
            if resp.len() >= core::mem::size_of::<FuseAttrOut>() {
                let out = unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseAttrOut) };
                return Metadata {
                    itype: attr_to_itype(out.attr.mode),
                    size: out.attr.size as usize,
                    inode_id: out.attr.ino,
                    permissions: (out.attr.mode & 0o7777) as u16,
                    uid: out.attr.uid,
                    gid: out.attr.gid,
                    rdev: out.attr.rdev as u64,
                };
            }
        }
        Metadata {
            itype: self.itype,
            size: 0,
            inode_id: self.nodeid,
            permissions: if self.itype == InodeType::Directory { 0o755 } else { 0o644 },
            uid: 0,
            gid: 0,
            rdev: 0,
        }
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        let mut payload = Vec::with_capacity(name.len() + 1);
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);

        let resp = self.session.call(FUSE_LOOKUP, self.nodeid, &payload)?;
        if resp.len() < core::mem::size_of::<FuseEntryOut>() {
            return Err(VfsError::IoError);
        }
        let entry = unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseEntryOut) };
        Ok(Arc::new(FuseInode {
            nodeid: entry.nodeid,
            itype: attr_to_itype(entry.attr.mode),
            session: self.session.clone(),
        }))
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let req = FuseReadIn {
            size: 32768,
            ..Default::default()
        };
        let resp = self.session.call(FUSE_READDIR, self.nodeid, as_bytes(&req))?;
        let mut entries = Vec::new();
        let mut pos = 0usize;
        let hdr_sz = core::mem::size_of::<FuseDirent>();
        while pos + hdr_sz <= resp.len() {
            let de = unsafe { core::ptr::read_unaligned(resp.as_ptr().add(pos) as *const FuseDirent) };
            let name_start = pos + hdr_sz;
            let name_end = name_start + de.namelen as usize;
            if name_end > resp.len() {
                break;
            }
            if let Ok(name) = core::str::from_utf8(&resp[name_start..name_end]) {
                entries.push(DirEntry {
                    name: String::from(name),
                    inode_id: de.ino,
                    itype: dirent_type_to_itype(de.typ),
                });
            }
            pos += fuse_dirent_align(hdr_sz + de.namelen as usize);
        }
        Ok(entries)
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let req = FuseReadIn {
            offset: offset as u64,
            size: buf.len() as u32,
            ..Default::default()
        };
        let resp = self.session.call(FUSE_READ, self.nodeid, as_bytes(&req))?;
        let n = resp.len().min(buf.len());
        buf[..n].copy_from_slice(&resp[..n]);
        Ok(n)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let req = FuseWriteIn {
            offset: offset as u64,
            size: buf.len() as u32,
            ..Default::default()
        };
        let hdr_bytes = as_bytes(&req);
        let mut payload = Vec::with_capacity(hdr_bytes.len() + buf.len());
        payload.extend_from_slice(hdr_bytes);
        payload.extend_from_slice(buf);

        let resp = self.session.call(FUSE_WRITE, self.nodeid, &payload)?;
        if resp.len() < core::mem::size_of::<FuseWriteOut>() {
            return Err(VfsError::IoError);
        }
        let out = unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseWriteOut) };
        Ok(out.size as usize)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let (opcode, hdr_bytes): (u32, Vec<u8>) = match itype {
            InodeType::Directory => (
                FUSE_MKDIR,
                as_bytes(&FuseMkdirIn { mode: 0o755, umask: 0 }).to_vec(),
            ),
            InodeType::File => (
                FUSE_CREATE,
                as_bytes(&FuseCreateIn {
                    flags: crate::fs::O_CREAT | crate::fs::O_RDWR,
                    mode: 0o100644,
                    umask: 0,
                    open_flags: 0,
                })
                .to_vec(),
            ),
            _ => return Err(VfsError::NotSupported),
        };
        let mut payload = hdr_bytes;
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        let resp = self.session.call(opcode, self.nodeid, &payload)?;
        if resp.len() < core::mem::size_of::<FuseEntryOut>() {
            return Err(VfsError::IoError);
        }
        let entry = unsafe { core::ptr::read_unaligned(resp.as_ptr() as *const FuseEntryOut) };
        Ok(Arc::new(FuseInode {
            nodeid: entry.nodeid,
            itype,
            session: self.session.clone(),
        }))
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let mut payload = Vec::with_capacity(name.len() + 1);
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        match self.session.call(FUSE_UNLINK, self.nodeid, &payload) {
            Ok(_) => Ok(()),
            Err(VfsError::IsADirectory) => {
                self.session.call(FUSE_RMDIR, self.nodeid, &payload)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn truncate(&self, len: usize) -> Result<(), VfsError> {
        let req = FuseSetattrIn {
            valid: FATTR_SIZE,
            size: len as u64,
            ..Default::default()
        };
        self.session.call(FUSE_SETATTR, self.nodeid, as_bytes(&req))?;
        Ok(())
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        let req = FuseSetattrIn {
            valid: FATTR_MODE,
            mode: mode as u32,
            ..Default::default()
        };
        self.session.call(FUSE_SETATTR, self.nodeid, as_bytes(&req))?;
        Ok(())
    }
}
