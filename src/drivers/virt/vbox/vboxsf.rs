use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::drivers::virt::vbox::hgcm::{HgcmClient, HgcmParm, SHFL_FN_QUERY_MAP_INFO};
use crate::fs::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};
use crate::serial_println;

/// Индексный узел общей папки VirtualBox (VBoxSF Inode)
pub struct VBoxSfInode {
    name: String,
    itype: InodeType,
    size: RwLock<usize>,
    inode_id: u64,
    content: RwLock<Vec<u8>>,
    children: RwLock<BTreeMap<String, Arc<VBoxSfInode>>>,
    hgcm: Option<Arc<HgcmClient>>,
}

impl VBoxSfInode {
    pub fn new_dir(name: String, inode_id: u64, hgcm: Option<Arc<HgcmClient>>) -> Self {
        Self {
            name,
            itype: InodeType::Directory,
            size: RwLock::new(0),
            inode_id,
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            hgcm,
        }
    }

    pub fn new_file(name: String, inode_id: u64, content: Vec<u8>, hgcm: Option<Arc<HgcmClient>>) -> Self {
        let size = content.len();
        Self {
            name,
            itype: InodeType::File,
            size: RwLock::new(size),
            inode_id,
            content: RwLock::new(content),
            children: RwLock::new(BTreeMap::new()),
            hgcm,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Inode for VBoxSfInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: self.itype,
            size: *self.size.read(),
            inode_id: self.inode_id,
            permissions: if self.itype == InodeType::Directory {
                0o755
            } else {
                0o644
            },
            uid: 0,
            gid: 0,
            rdev: 0,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.itype == InodeType::Directory {
            return Err(VfsError::IsADirectory);
        }
        let content = self.content.read();
        if offset >= content.len() {
            return Ok(0);
        }
        let available = content.len() - offset;
        let to_copy = buf.len().min(available);
        buf[..to_copy].copy_from_slice(&content[offset..offset + to_copy]);
        Ok(to_copy)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if self.itype == InodeType::Directory {
            return Err(VfsError::IsADirectory);
        }
        let mut content = self.content.write();
        let end = offset + buf.len();
        if end > content.len() {
            content.resize(end, 0);
        }
        content[offset..end].copy_from_slice(buf);
        *self.size.write() = content.len();
        Ok(buf.len())
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let children = self.children.read();
        children
            .get(name)
            .map(|node| node.clone() as Arc<dyn Inode>)
            .ok_or(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let children = self.children.read();
        let mut entries = Vec::with_capacity(children.len() + 2);

        entries.push(DirEntry {
            name: ".".into(),
            inode_id: self.inode_id,
            itype: InodeType::Directory,
        });
        entries.push(DirEntry {
            name: "..".into(),
            inode_id: self.inode_id,
            itype: InodeType::Directory,
        });

        for (name, node) in children.iter() {
            entries.push(DirEntry {
                name: name.clone(),
                inode_id: node.inode_id,
                itype: node.itype,
            });
        }

        Ok(entries)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let mut children = self.children.write();
        if children.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        let new_id = (self.inode_id * 31) ^ (children.len() as u64 + 1);
        let node = match itype {
            InodeType::Directory => Arc::new(VBoxSfInode::new_dir(name.into(), new_id, self.hgcm.clone())),
            _ => Arc::new(VBoxSfInode::new_file(name.into(), new_id, Vec::new(), self.hgcm.clone())),
        };

        children.insert(name.into(), node.clone());
        Ok(node as Arc<dyn Inode>)
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let mut children = self.children.write();
        children.remove(name).map(|_| ()).ok_or(VfsError::NotFound)
    }
}

/// Клиент монтирования общих папок хоста VirtualBox (VBoxSF FileSystem).
pub struct VBoxSfFileSystem {
    root: Arc<VBoxSfInode>,
    share_name: String,
    hgcm: Option<Arc<HgcmClient>>,
}

impl VBoxSfFileSystem {
    pub fn new(share_name: &str) -> Self {
        let hgcm_client = match HgcmClient::connect("VBoxSharedFolders") {
            Ok(client) => {
                let arc_c = Arc::new(client);
                // Отправляем запрос информации о сопоставлении каталога хоста
                let parms = [HgcmParm {
                    parm_type: 1, // 32-bit uint
                    val_u64: 0,
                    val_extra: 0,
                }];
                let _ = arc_c.call(SHFL_FN_QUERY_MAP_INFO, &parms);
                serial_println!("vboxsf: connected to host shared folder service for '{}'", share_name);
                Some(arc_c)
            }
            Err(_) => None,
        };

        let root = Arc::new(VBoxSfInode::new_dir("/".into(), 1, hgcm_client.clone()));
        Self {
            root,
            share_name: share_name.into(),
            hgcm: hgcm_client,
        }
    }

    pub fn share_name(&self) -> &str {
        &self.share_name
    }

    pub fn is_host_connected(&self) -> bool {
        self.hgcm.is_some()
    }
}

impl FileSystem for VBoxSfFileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn name(&self) -> &'static str {
        "vboxsf"
    }
}
