use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::virt::vbox::hgcm::{
    HgcmClient, HgcmParm, SHFL_FN_CLOSE, SHFL_FN_CREATE, SHFL_FN_LIST, SHFL_FN_MAP_FOLDER,
    SHFL_FN_READ, SHFL_FN_REMOVE, SHFL_FN_WRITE, VMMDEV_HGCM_PARM_TYPE_32BIT,
    VMMDEV_HGCM_PARM_TYPE_64BIT, VMMDEV_HGCM_PARM_TYPE_LINADDR,
};
use crate::fs::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};
use crate::serial_println;

const SHFL_CF_DIRECTORY: u32 = 0x0001;
const SHFL_CF_ACCESS_READWRITE: u32 = 0x0030;
const SHFL_CF_ACT_OPEN_IF_EXISTS: u32 = 0x0000;
const SHFL_CF_ACT_CREATE_IF_NEW: u32 = 0x0001;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ShflCreateParms {
    handle: u64,
    result: u32,
    create_flags: u32,
    file_size: u64,
}

#[repr(C)]
struct ShflStringHeader {
    size: u16,
    length: u16,
    utf8: [u8; 256],
}

impl ShflStringHeader {
    fn from_path(path: &str) -> Self {
        let mut s = Self {
            size: 260,
            length: 0,
            utf8: [0u8; 256],
        };
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        s.utf8[..len].copy_from_slice(&bytes[..len]);
        s.length = len as u16;
        s
    }
}

pub struct VBoxSfInode {
    rel_path: String,
    itype: InodeType,
    inode_id: u64,
    root_handle: u32,
    hgcm: Option<Arc<HgcmClient>>,
    io_lock: Mutex<()>,
}

impl VBoxSfInode {
    fn require_hgcm(&self) -> Result<&Arc<HgcmClient>, VfsError> {
        self.hgcm.as_ref().ok_or(VfsError::NotSupported)
    }

    fn shfl_open(&self, create: bool, is_dir: bool) -> Result<(u64, u64), VfsError> {
        let client = self.require_hgcm()?;
        let shfl_path = ShflStringHeader::from_path(&self.rel_path);
        let mut create_parms = ShflCreateParms {
            handle: 0,
            result: 0,
            create_flags: SHFL_CF_ACCESS_READWRITE
                | if create { SHFL_CF_ACT_CREATE_IF_NEW } else { SHFL_CF_ACT_OPEN_IF_EXISTS }
                | if is_dir { SHFL_CF_DIRECTORY } else { 0 },
            file_size: 0,
        };
        let mut parms = [
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: self.root_handle as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: &shfl_path as *const _ as u64,
                val_extra: core::mem::size_of::<ShflStringHeader>() as u32,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: &mut create_parms as *mut _ as u64,
                val_extra: core::mem::size_of::<ShflCreateParms>() as u32,
            },
        ];
        let rc = client
            .call(SHFL_FN_CREATE, &mut parms)
            .map_err(|_| VfsError::IoError)?;
        if rc < 0 || create_parms.handle == 0 {
            return Err(VfsError::NotFound);
        }
        Ok((create_parms.handle, create_parms.file_size))
    }

    fn shfl_close(&self, handle: u64) {
        if let Some(ref client) = self.hgcm {
            let mut parms = [
                HgcmParm {
                    parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                    val_u64: self.root_handle as u64,
                    val_extra: 0,
                },
                HgcmParm {
                    parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                    val_u64: handle,
                    val_extra: 0,
                },
            ];
            let _ = client.call(SHFL_FN_CLOSE, &mut parms);
        }
    }
}

impl Inode for VBoxSfInode {
    fn metadata(&self) -> Metadata {
        let size = if self.itype == InodeType::File {
            self.shfl_open(false, false)
                .map(|(h, sz)| {
                    self.shfl_close(h);
                    sz as usize
                })
                .unwrap_or(0)
        } else {
            0
        };
        Metadata {
            itype: self.itype,
            size,
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
        let _guard = self.io_lock.lock();
        let client = self.require_hgcm()?;
        let (handle, _) = self.shfl_open(false, false)?;
        let mut parms = [
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: self.root_handle as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                val_u64: handle,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                val_u64: offset as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: buf.len() as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: buf.as_mut_ptr() as u64,
                val_extra: buf.len() as u32,
            },
        ];
        let rc = client.call(SHFL_FN_READ, &mut parms);
        self.shfl_close(handle);
        if rc.unwrap_or(-1) < 0 {
            return Err(VfsError::IoError);
        }
        Ok((parms[3].val_u64 as usize).min(buf.len()))
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if self.itype == InodeType::Directory {
            return Err(VfsError::IsADirectory);
        }
        let _guard = self.io_lock.lock();
        let client = self.require_hgcm()?;
        let (handle, _) = self.shfl_open(false, false)?;
        let mut parms = [
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: self.root_handle as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                val_u64: handle,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                val_u64: offset as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: buf.len() as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: buf.as_ptr() as u64,
                val_extra: buf.len() as u32,
            },
        ];
        let rc = client.call(SHFL_FN_WRITE, &mut parms);
        self.shfl_close(handle);
        if rc.unwrap_or(-1) < 0 {
            return Err(VfsError::IoError);
        }
        Ok(parms[3].val_u64 as usize)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let child_path = if self.rel_path.is_empty() || self.rel_path == "/" {
            String::from(name)
        } else {
            format!("{}/{}", self.rel_path, name)
        };
        let probe_file = VBoxSfInode {
            rel_path: child_path.clone(),
            itype: InodeType::File,
            inode_id: (self.inode_id * 131) ^ name.len() as u64,
            root_handle: self.root_handle,
            hgcm: self.hgcm.clone(),
            io_lock: Mutex::new(()),
        };
        if let Ok((h, _)) = probe_file.shfl_open(false, false) {
            probe_file.shfl_close(h);
            return Ok(Arc::new(probe_file));
        }
        let probe_dir = VBoxSfInode {
            itype: InodeType::Directory,
            io_lock: Mutex::new(()),
            ..probe_file
        };
        if let Ok((h, _)) = probe_dir.shfl_open(false, true) {
            probe_dir.shfl_close(h);
            return Ok(Arc::new(probe_dir));
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        if self.itype != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let client = self.require_hgcm()?;
        let (handle, _) = self.shfl_open(false, true)?;
        let mut list_buf = vec![0u8; 4096];
        let mut parms = [
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: self.root_handle as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_64BIT,
                val_u64: handle,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: 0,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: list_buf.len() as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: list_buf.as_mut_ptr() as u64,
                val_extra: list_buf.len() as u32,
            },
        ];
        let _ = client.call(SHFL_FN_LIST, &mut parms);
        self.shfl_close(handle);

        let mut entries = Vec::new();
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
        Ok(entries)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let child_path = if self.rel_path.is_empty() || self.rel_path == "/" {
            String::from(name)
        } else {
            format!("{}/{}", self.rel_path, name)
        };
        let node = VBoxSfInode {
            rel_path: child_path,
            itype,
            inode_id: (self.inode_id * 131) ^ name.len() as u64,
            root_handle: self.root_handle,
            hgcm: self.hgcm.clone(),
            io_lock: Mutex::new(()),
        };
        let (h, _) = node.shfl_open(true, itype == InodeType::Directory)?;
        node.shfl_close(h);
        Ok(Arc::new(node))
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let client = self.require_hgcm()?;
        let child_path = if self.rel_path.is_empty() || self.rel_path == "/" {
            String::from(name)
        } else {
            format!("{}/{}", self.rel_path, name)
        };
        let shfl_path = ShflStringHeader::from_path(&child_path);
        let mut parms = [
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: self.root_handle as u64,
                val_extra: 0,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                val_u64: &shfl_path as *const _ as u64,
                val_extra: core::mem::size_of::<ShflStringHeader>() as u32,
            },
            HgcmParm {
                parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                val_u64: 0,
                val_extra: 0,
            },
        ];
        let rc = client
            .call(SHFL_FN_REMOVE, &mut parms)
            .map_err(|_| VfsError::IoError)?;
        if rc < 0 {
            Err(VfsError::NotFound)
        } else {
            Ok(())
        }
    }
}

pub struct VBoxSfFileSystem {
    root: Arc<VBoxSfInode>,
    share_name: String,
    hgcm: Option<Arc<HgcmClient>>,
}

impl VBoxSfFileSystem {
    pub fn new(share_name: &str) -> Self {
        let mut root_handle = 0u32;
        let hgcm_client = match HgcmClient::connect("VBoxSharedFolders") {
            Ok(client) => {
                let arc_c = Arc::new(client);
                let shfl_name = ShflStringHeader::from_path(share_name);
                let mut parms = [
                    HgcmParm {
                        parm_type: VMMDEV_HGCM_PARM_TYPE_LINADDR,
                        val_u64: &shfl_name as *const _ as u64,
                        val_extra: core::mem::size_of::<ShflStringHeader>() as u32,
                    },
                    HgcmParm {
                        parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                        val_u64: 0,
                        val_extra: 0,
                    },
                    HgcmParm {
                        parm_type: VMMDEV_HGCM_PARM_TYPE_32BIT,
                        val_u64: 0,
                        val_extra: 0,
                    },
                ];
                if arc_c.call(SHFL_FN_MAP_FOLDER, &mut parms).unwrap_or(-1) >= 0 {
                    root_handle = parms[1].val_u64 as u32;
                    serial_println!(
                        "vboxsf: mapped host folder '{}' (root={})",
                        share_name,
                        root_handle
                    );
                }
                Some(arc_c)
            }
            Err(_) => None,
        };

        let root = Arc::new(VBoxSfInode {
            rel_path: String::from("/"),
            itype: InodeType::Directory,
            inode_id: 1,
            root_handle,
            hgcm: hgcm_client.clone(),
            io_lock: Mutex::new(()),
        });
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
