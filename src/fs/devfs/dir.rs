use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use crate::fs::vfs::{DirEntry, Inode, InodeType, Metadata, VfsError};

static NEXT_DIR_INODE_ID: AtomicU64 = AtomicU64::new(0xDE00_8001);

fn next_dir_inode_id() -> u64 {
    NEXT_DIR_INODE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Директория файловой системы устройств DevFS (/dev, /dev/input, /dev/pts).
/// Обеспечивает динамическую регистрацию и перечисление символьных и блочных узлов.
pub struct DevDirInode {
    pub(crate) inode_id: u64,
    pub(crate) children: RwLock<BTreeMap<String, Arc<dyn Inode>>>,
}

impl DevDirInode {
    pub fn new() -> Self {
        Self {
            inode_id: next_dir_inode_id(),
            children: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn insert(&self, name: &str, node: Arc<dyn Inode>) {
        self.children.write().insert(String::from(name), node);
    }
}

impl Inode for DevDirInode {
    fn metadata(&self) -> Metadata {
        let count = self.children.read().len();
        Metadata {
            itype: InodeType::Directory,
            size: count * 32,
            inode_id: self.inode_id,
            permissions: 0o755,
            uid: 0,
            gid: 0,
            rdev: 0,
        }
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        let guard = self.children.read();
        guard.get(name).cloned().ok_or(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let guard = self.children.read();
        let mut entries = Vec::with_capacity(guard.len());
        for (name, node) in guard.iter() {
            let meta = node.metadata();
            entries.push(DirEntry {
                name: name.clone(),
                inode_id: meta.inode_id,
                itype: meta.itype,
            });
        }
        Ok(entries)
    }

    fn insert_child(&self, name: &str, node: Arc<dyn Inode>) -> Result<(), VfsError> {
        self.children.write().insert(String::from(name), node);
        Ok(())
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let mut guard = self.children.write();
        if guard.remove(name).is_some() {
            Ok(())
        } else {
            Err(VfsError::NotFound)
        }
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let mut guard = self.children.write();
        if guard.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        let new_node: Arc<dyn Inode> = match itype {
            InodeType::Directory => Arc::new(DevDirInode::new()),
            InodeType::Symlink => Arc::new(crate::fs::RamInode::new_symlink("")),
            InodeType::File => Arc::new(crate::fs::RamInode::new_file()),
            _ => return Err(VfsError::NotSupported),
        };

        guard.insert(String::from(name), new_node.clone());
        Ok(new_node)
    }
}
