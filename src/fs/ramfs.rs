use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::min;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use spin::RwLock;

use super::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};

static NEXT_INODE_ID: AtomicU64 = AtomicU64::new(1);

fn next_inode_id() -> u64 {
    NEXT_INODE_ID.fetch_add(1, Ordering::Relaxed)
}

pub enum RamNodeData {
    File(RwLock<Vec<u8>>),
    Directory {
        children: RwLock<BTreeMap<String, Arc<dyn Inode>>>,
        case_insensitive: RwLock<bool>,
    },
    Symlink(RwLock<String>),
}

// Inode для файловой системы в оперативной памяти.
// Потокобезопасен благодаря гранулярным spin::RwLock внутри данных узла.
pub struct RamInode {
    inode_id: u64,
    itype: InodeType,
    permissions: AtomicU16,
    uid: AtomicU32,
    gid: AtomicU32,
    data: RamNodeData,
}

impl RamInode {
    pub fn new_file() -> Self {
        let umask = crate::task::current_thread_umask();
        let mode = (0o666 & !umask) as u16;
        Self {
            inode_id: next_inode_id(),
            itype: InodeType::File,
            permissions: AtomicU16::new(mode),
            uid: AtomicU32::new(crate::task::current_thread_euid()),
            gid: AtomicU32::new(crate::task::current_thread_egid()),
            data: RamNodeData::File(RwLock::new(Vec::new())),
        }
    }

    pub fn new_dir(case_insensitive: bool) -> Self {
        let umask = crate::task::current_thread_umask();
        let mode = (0o777 & !umask) as u16;
        Self {
            inode_id: next_inode_id(),
            itype: InodeType::Directory,
            permissions: AtomicU16::new(mode),
            uid: AtomicU32::new(crate::task::current_thread_euid()),
            gid: AtomicU32::new(crate::task::current_thread_egid()),
            data: RamNodeData::Directory {
                children: RwLock::new(BTreeMap::new()),
                case_insensitive: RwLock::new(case_insensitive),
            },
        }
    }

    pub fn new_symlink(target: &str) -> Self {
        Self {
            inode_id: next_inode_id(),
            itype: InodeType::Symlink,
            permissions: AtomicU16::new(0o777),
            uid: AtomicU32::new(crate::task::current_thread_euid()),
            gid: AtomicU32::new(crate::task::current_thread_egid()),
            data: RamNodeData::Symlink(RwLock::new(String::from(target))),
        }
    }

    // Включает или отключает регистронезависимый поиск в директории (для дерева NT)
    pub fn set_case_insensitive(&self, enabled: bool) {
        if let RamNodeData::Directory { case_insensitive, .. } = &self.data {
            *case_insensitive.write() = enabled;
        }
    }

    // Вставляет готовый Inode напрямую в таблицу дочерних элементов директории
    pub fn insert_child(&self, name: &str, node: Arc<dyn Inode>) -> Result<(), VfsError> {
        match &self.data {
            RamNodeData::Directory { children, .. } => {
                let mut map = children.write();
                map.insert(String::from(name), node);
                Ok(())
            }
            _ => Err(VfsError::NotADirectory),
        }
    }
}

impl Inode for RamInode {
    fn metadata(&self) -> Metadata {
        let size = match &self.data {
            RamNodeData::File(d) => d.read().len(),
            RamNodeData::Directory { children, .. } => children.read().len() * 32,
            RamNodeData::Symlink(target) => target.read().len(),
        };

        Metadata {
            itype: self.itype,
            size,
            inode_id: self.inode_id,
            permissions: self.permissions.load(Ordering::Relaxed),
            uid: self.uid.load(Ordering::Relaxed),
            gid: self.gid.load(Ordering::Relaxed),
            rdev: 0,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let data = match &self.data {
            RamNodeData::File(d) => d.read(),
            RamNodeData::Directory { .. } => return Err(VfsError::IsADirectory),
            RamNodeData::Symlink(_) => return Err(VfsError::NotSupported),
        };

        if offset >= data.len() {
            return Ok(0);
        }

        let slice = &data[offset..];
        let bytes_to_read = min(slice.len(), buf.len());
        buf[..bytes_to_read].copy_from_slice(&slice[..bytes_to_read]);
        Ok(bytes_to_read)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        match &self.data {
            RamNodeData::File(d) => {
                let mut data = d.write();
                // Быстрый путь для новой записи с начала файла: избегаем двойной работы
                // (resize заполняет буфер нулями, которые затем перезаписываются copy_from_slice)
                if offset == 0 && data.is_empty() {
                    data.extend_from_slice(buf);
                    return Ok(buf.len());
                }
                let end = offset.checked_add(buf.len()).ok_or(VfsError::IoError)?;
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[offset..end].copy_from_slice(buf);
                Ok(buf.len())
            }
            RamNodeData::Symlink(target) => {
                let s = core::str::from_utf8(buf).map_err(|_| VfsError::InvalidPath)?;
                *target.write() = String::from(s);
                Ok(buf.len())
            }
            RamNodeData::Directory { .. } => Err(VfsError::IsADirectory),
        }
    }

    fn truncate(&self, len: usize) -> Result<(), VfsError> {
        match &self.data {
            RamNodeData::File(d) => {
                let mut data = d.write();
                data.resize(len, 0);
                Ok(())
            }
            _ => Err(VfsError::NotSupported),
        }
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        let (children, case_insensitive) = match &self.data {
            RamNodeData::Directory {
                children,
                case_insensitive,
            } => (children.read(), *case_insensitive.read()),
            _ => return Err(VfsError::NotADirectory),
        };

        // Сначала быстрый точный поиск по BTreeMap
        if let Some(node) = children.get(name) {
            return Ok(node.clone());
        }

        // Если директория помечена регистронезависимой (ветка /win), ищем без учета регистра
        if case_insensitive {
            for (key, node) in children.iter() {
                if key.eq_ignore_ascii_case(name) {
                    return Ok(node.clone());
                }
            }
        }

        Err(VfsError::NotFound)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let (mut children, case_insensitive) = match &self.data {
            RamNodeData::Directory {
                children,
                case_insensitive,
            } => (children.write(), *case_insensitive.read()),
            _ => return Err(VfsError::NotADirectory),
        };

        if children.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        if case_insensitive && children.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            return Err(VfsError::AlreadyExists);
        }

        // Подсистема Windows и все её дочерние каталоги регистронезависимы
        let child_case_insensitive = case_insensitive || name.eq_ignore_ascii_case("win");

        let new_node: Arc<dyn Inode> = match itype {
            InodeType::File => Arc::new(RamInode::new_file()),
            InodeType::Directory => Arc::new(RamInode::new_dir(child_case_insensitive)),
            InodeType::Symlink => Arc::new(RamInode::new_symlink("")),
            _ => return Err(VfsError::NotSupported),
        };

        children.insert(String::from(name), new_node.clone());
        Ok(new_node)
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let (mut children, case_insensitive) = match &self.data {
            RamNodeData::Directory {
                children,
                case_insensitive,
            } => (children.write(), *case_insensitive.read()),
            _ => return Err(VfsError::NotADirectory),
        };

        if children.remove(name).is_some() {
            return Ok(());
        }

        if case_insensitive {
            let key_to_remove = children
                .keys()
                .find(|k| k.eq_ignore_ascii_case(name))
                .cloned();
            if let Some(k) = key_to_remove {
                children.remove(&k);
                return Ok(());
            }
        }

        Err(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let children = match &self.data {
            RamNodeData::Directory { children, .. } => children.read(),
            _ => return Err(VfsError::NotADirectory),
        };

        let mut entries = Vec::with_capacity(children.len());
        for (name, node) in children.iter() {
            let meta = node.metadata();
            entries.push(DirEntry {
                name: name.clone(),
                inode_id: meta.inode_id,
                itype: meta.itype,
            });
        }

        Ok(entries)
    }

    fn read_link(&self) -> Result<String, VfsError> {
        match &self.data {
            RamNodeData::Symlink(target) => Ok(target.read().clone()),
            _ => Err(VfsError::NotSupported),
        }
    }

    fn insert_child(&self, name: &str, node: Arc<dyn Inode>) -> Result<(), VfsError> {
        self.insert_child(name, node)
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
}

// Корневой инстанс файловой системы в ОЗУ
pub struct RamFs {
    root: Arc<RamInode>,
}

impl RamFs {
    pub fn new() -> Self {
        Self {
            root: Arc::new(RamInode::new_dir(false)),
        }
    }
}

impl FileSystem for RamFs {
    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn name(&self) -> &'static str {
        "ramfs"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_ramfs_file_io() {
        let file = RamInode::new_file();
        let payload = b"Hello RamFS!";
        let written = file.write_at(0, payload).expect("write failed");
        assert_eq!(written, payload.len());

        let mut buf = [0u8; 32];
        let read = file.read_at(0, &mut buf).expect("read failed");
        assert_eq!(read, payload.len());
        assert_eq!(&buf[..read], payload);
    }

    #[test_case]
    fn test_ramfs_case_insensitive() {
        let dir = RamInode::new_dir(true);
        let _ = dir.create("System32", InodeType::Directory).expect("create dir");
        let found = dir.lookup("system32").expect("case-insensitive lookup failed");
        assert_eq!(found.metadata().itype, InodeType::Directory);
    }

    #[test_case]
    fn test_ramfs_symlink() {
        let link = RamInode::new_symlink("/home/user");
        assert_eq!(link.read_link().expect("read_link failed"), "/home/user");
    }
}
