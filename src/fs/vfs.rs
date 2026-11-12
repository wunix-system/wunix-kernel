use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    File,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub itype: InodeType,
    pub size: usize,
    pub inode_id: u64,
    pub permissions: u16,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    IsADirectory,
    NotADirectory,
    PermissionDenied,
    AlreadyExists,
    InvalidPath,
    IoError,
    NotSupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: u64,
    pub itype: InodeType,
}

// Базовый интерфейс индексного узла VFS.
// Реализации узлов (RamFS, DevFS, ProcFS) переопределяют методы,
// релевантные для своего типа, а для остальных получают дефолтные ошибки.
pub trait Inode: Send + Sync {
    fn metadata(&self) -> Metadata;

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotSupported)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotSupported)
    }

    fn truncate(&self, _len: usize) -> Result<(), VfsError> {
        Err(VfsError::NotSupported)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        Err(VfsError::NotADirectory)
    }

    fn create(&self, _name: &str, _itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        Err(VfsError::NotADirectory)
    }

    fn unlink(&self, _name: &str) -> Result<(), VfsError> {
        Err(VfsError::NotADirectory)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        Err(VfsError::NotADirectory)
    }

    fn read_link(&self) -> Result<String, VfsError> {
        Err(VfsError::NotSupported)
    }

    fn insert_child(&self, _name: &str, _node: Arc<dyn Inode>) -> Result<(), VfsError> {
        Err(VfsError::NotADirectory)
    }

    fn chmod(&self, _mode: u16) -> Result<(), VfsError> {
        Err(VfsError::NotSupported)
    }

    fn chown(&self, _uid: u32, _gid: u32) -> Result<(), VfsError> {
        Err(VfsError::NotSupported)
    }

    fn ioctl(&self, _cmd: u32, _arg: u64) -> Result<u64, i64> {
        Err(-25)
    }

    fn open_handle(&self, _flags: u32, _path: Option<alloc::string::String>) -> Option<Arc<dyn crate::fs::handle::FileHandle>> {
        None
    }
}

// Корневой контракт конкретной файловой системы.
// Позволяет монтировать RamFS, Ext2 или DevFS в произвольные узлы дерева.
pub trait FileSystem: Send + Sync {
    fn root(&self) -> Arc<dyn Inode>;
    fn name(&self) -> &'static str;
}
