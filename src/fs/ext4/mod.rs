//! Полнофункциональный драйвер файловой системы Ext4 с поддержкой журнала JBD2.
//!
//! Реализует стандарты Linux Ext4:
//! - Дерево экстентов (Extent Tree) для быстрого маппинга смещений на блоки
//! - Дескрипторы групп блоков и дисковые битовые карты аллокации
//! - Транзакционный журнал JBD2 для защиты метаданных от сбоев питания
//! - Полный контракт VFS (Inode, FileSystem, POSIX chmod/chown)

pub mod block_group;
pub mod dir;
pub mod extent;
pub mod inode;
pub mod journal;
pub mod superblock;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::BlockDevice;
use super::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};
use self::block_group::{allocate_bit, BlockGroupDesc};
use self::dir::{find_entry, insert_entry, remove_entry, to_vfs_entries};
use self::extent::{append_extent, lookup_logical_block};
use self::inode::{DiskInode, EXT4_INODE_ROOT_INO};
use self::journal::Ext4Journal;
use self::superblock::{Superblock, EXT4_SUPERBLOCK_OFFSET};

pub struct Ext4Inner {
    pub dev: Arc<Mutex<dyn BlockDevice>>,
    pub sb: Mutex<Superblock>,
    pub group_descs: Mutex<Vec<BlockGroupDesc>>,
    pub journal: Mutex<Option<Ext4Journal>>,
}

impl Ext4Inner {
    pub fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        let sec = block_num * 8;
        self.dev.lock().read_sectors(sec, buf).map_err(|_| VfsError::IoError)
    }

    pub fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.write_metadata_block(block_num, buf)
    }

    pub fn write_metadata_block(&self, block_num: u64, buf: &[u8]) -> Result<(), VfsError> {
        let mut guard = self.journal.lock();
        if let Some(journal) = guard.as_mut() {
            journal.commit_transaction(&[(block_num, buf)])
        } else {
            let sec = block_num * 8;
            self.dev.lock().write_sectors(sec, buf).map_err(|_| VfsError::IoError)
        }
    }

    pub fn write_data_block(&self, block_num: u64, buf: &[u8]) -> Result<(), VfsError> {
        let sec = block_num * 8;
        self.dev.lock().write_sectors(sec, buf).map_err(|_| VfsError::IoError)
    }

    pub fn read_inode(&self, ino: u32) -> Result<DiskInode, VfsError> {
        if ino == 0 {
            return Err(VfsError::NotFound);
        }
        let sb = *self.sb.lock();
        let group = ((ino - 1) / sb.inodes_per_group) as usize;
        let index_in_group = ((ino - 1) % sb.inodes_per_group) as usize;

        let table_block = {
            let groups = self.group_descs.lock();
            if group >= groups.len() {
                return Err(VfsError::IoError);
            }
            groups[group].inode_table as u64
        };

        let inode_size = sb.inode_size as usize;
        let byte_offset = index_in_group * inode_size;
        let block_offset = byte_offset / (sb.block_size as usize);
        let offset_in_block = byte_offset % (sb.block_size as usize);

        let mut buf = vec![0u8; sb.block_size as usize];
        self.read_block(table_block + block_offset as u64, &mut buf)?;

        let slice = &buf[offset_in_block..offset_in_block + inode_size];
        Ok(DiskInode::parse(slice))
    }

    pub fn write_inode(&self, ino: u32, inode: &DiskInode) -> Result<(), VfsError> {
        let sb = *self.sb.lock();
        let group = ((ino - 1) / sb.inodes_per_group) as usize;
        let index_in_group = ((ino - 1) % sb.inodes_per_group) as usize;

        let table_block = {
            let groups = self.group_descs.lock();
            if group >= groups.len() {
                return Err(VfsError::IoError);
            }
            groups[group].inode_table as u64
        };

        let inode_size = sb.inode_size as usize;
        let byte_offset = index_in_group * inode_size;
        let block_offset = byte_offset / (sb.block_size as usize);
        let offset_in_block = byte_offset % (sb.block_size as usize);

        let mut buf = vec![0u8; sb.block_size as usize];
        let target_block = table_block + block_offset as u64;
        self.read_block(target_block, &mut buf)?;

        inode.write_to_slice(&mut buf[offset_in_block..offset_in_block + inode_size]);
        self.write_block(target_block, &buf)
    }

    pub fn allocate_block(&self) -> Result<u64, VfsError> {
        let mut groups = self.group_descs.lock();
        let mut sb = self.sb.lock();

        for (grp_idx, desc) in groups.iter_mut().enumerate() {
            if desc.free_blocks_count > 0 {
                let mut bitmap = vec![0u8; sb.block_size as usize];
                self.read_block(desc.block_bitmap as u64, &mut bitmap)?;

                let bit = allocate_bit(&mut bitmap, sb.blocks_per_group as usize)?;
                desc.free_blocks_count = desc.free_blocks_count.saturating_sub(1);
                sb.free_blocks_count = sb.free_blocks_count.saturating_sub(1);

                self.write_block(desc.block_bitmap as u64, &bitmap)?;

                let block_num = (grp_idx as u64) * (sb.blocks_per_group as u64) + (bit as u64) + (sb.first_data_block as u64);
                return Ok(block_num);
            }
        }
        Err(VfsError::IoError)
    }

    pub fn allocate_inode(&self, itype: InodeType) -> Result<u32, VfsError> {
        let mut groups = self.group_descs.lock();
        let mut sb = self.sb.lock();

        for (grp_idx, desc) in groups.iter_mut().enumerate() {
            if desc.free_inodes_count > 0 {
                let mut bitmap = vec![0u8; sb.block_size as usize];
                self.read_block(desc.inode_bitmap as u64, &mut bitmap)?;

                let bit = allocate_bit(&mut bitmap, sb.inodes_per_group as usize)?;
                desc.free_inodes_count = desc.free_inodes_count.saturating_sub(1);
                if itype == InodeType::Directory {
                    desc.used_dirs_count += 1;
                }
                sb.free_inodes_count = sb.free_inodes_count.saturating_sub(1);

                self.write_block(desc.inode_bitmap as u64, &bitmap)?;

                let ino = (grp_idx as u32) * sb.inodes_per_group + (bit as u32) + 1;
                return Ok(ino);
            }
        }
        Err(VfsError::IoError)
    }
}

pub struct Ext4FileSystem {
    pub inner: Arc<Ext4Inner>,
    pub root_inode: Arc<Ext4Inode>,
}

impl Ext4FileSystem {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        let mut sb_buf = vec![0u8; 1024];
        dev.lock()
            .read_sectors(EXT4_SUPERBLOCK_OFFSET / 512, &mut sb_buf)
            .map_err(|_| VfsError::IoError)?;

        let sb = Superblock::parse(&sb_buf)?;

        let group_count = sb.group_count();
        let desc_block = if sb.block_size == 1024 { 2 } else { 1 };
        let mut desc_buf = vec![0u8; sb.block_size as usize];
        dev.lock()
            .read_sectors(desc_block * 8, &mut desc_buf)
            .map_err(|_| VfsError::IoError)?;

        let mut groups = Vec::with_capacity(group_count);
        for i in 0..group_count {
            let offset = i * 32;
            if offset + 32 <= desc_buf.len() {
                groups.push(BlockGroupDesc::parse(&desc_buf[offset..offset + 32]));
            }
        }

        let journal = if sb.has_journal() {
            // Читаем инод журнала (обычно inode 8)
            let mut j_inst = None;
            if let Some(desc) = groups.first() {
                let mut tbl_buf = vec![0u8; sb.block_size as usize];
                if dev.lock().read_sectors((desc.inode_table as u64) * 8, &mut tbl_buf).is_ok() {
                    let j_offset = (sb.journal_inum as usize - 1) * (sb.inode_size as usize);
                    if j_offset + (sb.inode_size as usize) <= tbl_buf.len() {
                        let j_dinode = DiskInode::parse(&tbl_buf[j_offset..j_offset + (sb.inode_size as usize)]);
                        if let Ok(Some(first_block)) = lookup_logical_block(&j_dinode.block, 0) {
                            j_inst = Ext4Journal::open(dev.clone(), first_block).ok();
                        }
                    }
                }
            }
            j_inst
        } else {
            None
        };

        let inner = Arc::new(Ext4Inner {
            dev,
            sb: Mutex::new(sb),
            group_descs: Mutex::new(groups),
            journal: Mutex::new(journal),
        });

        let root_dinode = inner.read_inode(EXT4_INODE_ROOT_INO)?;
        let root_inode = Arc::new(Ext4Inode {
            inner: inner.clone(),
            ino: EXT4_INODE_ROOT_INO,
            dinode: Mutex::new(root_dinode),
        });

        Ok(Self { inner, root_inode })
    }
}

impl FileSystem for Ext4FileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.root_inode.clone()
    }

    fn name(&self) -> &'static str {
        "ext4"
    }
}

pub struct Ext4Inode {
    inner: Arc<Ext4Inner>,
    ino: u32,
    dinode: Mutex<DiskInode>,
}

impl Inode for Ext4Inode {
    fn metadata(&self) -> Metadata {
        self.dinode.lock().to_metadata(self.ino as u64)
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let dinode = self.dinode.lock();
        if offset >= dinode.size as usize {
            return Ok(0);
        }
        let to_read = buf.len().min((dinode.size as usize) - offset);
        let block_size = self.inner.sb.lock().block_size as usize;

        let mut read_bytes = 0;
        let mut cur_offset = offset;
        let mut temp_block = vec![0u8; block_size];

        while read_bytes < to_read {
            let logical_block = (cur_offset / block_size) as u32;
            let block_internal_offset = cur_offset % block_size;
            let chunk = (block_size - block_internal_offset).min(to_read - read_bytes);

            if let Some(phys_block) = lookup_logical_block(&dinode.block, logical_block)? {
                self.inner.read_block(phys_block, &mut temp_block)?;
                buf[read_bytes..read_bytes + chunk]
                    .copy_from_slice(&temp_block[block_internal_offset..block_internal_offset + chunk]);
            } else {
                buf[read_bytes..read_bytes + chunk].fill(0);
            }

            read_bytes += chunk;
            cur_offset += chunk;
        }

        Ok(read_bytes)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let mut dinode = self.dinode.lock();
        let block_size = self.inner.sb.lock().block_size as usize;
        let mut written = 0;
        let mut cur_offset = offset;
        let mut temp_block = vec![0u8; block_size];

        while written < buf.len() {
            let logical_block = (cur_offset / block_size) as u32;
            let block_internal_offset = cur_offset % block_size;
            let chunk = (block_size - block_internal_offset).min(buf.len() - written);

            let mut is_new_block = false;
            let phys_block = match lookup_logical_block(&dinode.block, logical_block)? {
                Some(p) => p,
                None => {
                    let new_block = self.inner.allocate_block()?;
                    append_extent(&mut dinode.block, logical_block, new_block, 1)?;
                    dinode.blocks_512 += (block_size / 512) as u64;
                    temp_block.fill(0);
                    is_new_block = true;
                    new_block
                }
            };

            if chunk < block_size && !is_new_block {
                self.inner.read_block(phys_block, &mut temp_block)?;
            }
            temp_block[block_internal_offset..block_internal_offset + chunk]
                .copy_from_slice(&buf[written..written + chunk]);
            self.inner.write_data_block(phys_block, &temp_block)?;

            written += chunk;
            cur_offset += chunk;
        }

        if cur_offset as u64 > dinode.size {
            dinode.size = cur_offset as u64;
        }
        self.inner.write_inode(self.ino, &dinode)?;
        Ok(written)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        let dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let block_size = self.inner.sb.lock().block_size as usize;
        let mut temp_block = vec![0u8; block_size];

        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;
        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if let Some(entry) = find_entry(&temp_block, name) {
                    let child_dinode = self.inner.read_inode(entry.inode)?;
                    return Ok(Arc::new(Ext4Inode {
                        inner: self.inner.clone(),
                        ino: entry.inode,
                        dinode: Mutex::new(child_dinode),
                    }));
                }
            }
        }
        Err(VfsError::NotFound)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let mut dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let block_size = self.inner.sb.lock().block_size as usize;
        let new_ino = self.inner.allocate_inode(itype)?;

        let (perms, uid, gid) = match itype {
            InodeType::Directory => (0o755, 0, 0),
            _ => (0o644, 0, 0),
        };
        let mut new_dinode = DiskInode::new_empty(itype, perms, uid, gid);

        if itype == InodeType::Directory {
            // Выделяем начальный блок для каталога с записями . и ..
            let dir_block = self.inner.allocate_block()?;
            append_extent(&mut new_dinode.block, 0, dir_block, 1)?;
            new_dinode.size = block_size as u64;
            new_dinode.blocks_512 = (block_size / 512) as u64;

            let mut init_dir = vec![0u8; block_size];
            dir::init_dir_block(&mut init_dir, new_ino, self.ino);
            self.inner.write_block(dir_block, &init_dir)?;
        }

        self.inner.write_inode(new_ino, &new_dinode)?;

        // Вставляем запись в родительский каталог
        let mut inserted = false;
        let mut temp_block = vec![0u8; block_size];
        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;

        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if insert_entry(&mut temp_block, new_ino, name, itype).is_ok() {
                    self.inner.write_block(phys, &temp_block)?;
                    inserted = true;
                    break;
                }
            }
        }

        if !inserted {
            // Выделяем новый блок для родительского каталога
            let new_dir_block = self.inner.allocate_block()?;
            let logical_idx = total_blocks as u32;
            append_extent(&mut dinode.block, logical_idx, new_dir_block, 1)?;
            dinode.size += block_size as u64;
            dinode.blocks_512 += (block_size / 512) as u64;

            dir::init_empty_dir_block(&mut temp_block);
            insert_entry(&mut temp_block, new_ino, name, itype)?;
            self.inner.write_block(new_dir_block, &temp_block)?;
            self.inner.write_inode(self.ino, &dinode)?;
        }

        Ok(Arc::new(Ext4Inode {
            inner: self.inner.clone(),
            ino: new_ino,
            dinode: Mutex::new(new_dinode),
        }))
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let dinode = self.dinode.lock();
        let block_size = self.inner.sb.lock().block_size as usize;
        let mut temp_block = vec![0u8; block_size];
        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;

        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if remove_entry(&mut temp_block, name).is_ok() {
                    self.inner.write_block(phys, &temp_block)?;
                    return Ok(());
                }
            }
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let block_size = self.inner.sb.lock().block_size as usize;
        let mut temp_block = vec![0u8; block_size];
        let mut result = Vec::new();

        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;
        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                result.extend(to_vfs_entries(&temp_block));
            }
        }

        Ok(result)
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        let mut dinode = self.dinode.lock();
        dinode.mode = (dinode.mode & 0o170000) | (mode & 0o7777);
        self.inner.write_inode(self.ino, &dinode)
    }

    fn chown(&self, uid: u32, gid: u32) -> Result<(), VfsError> {
        let mut dinode = self.dinode.lock();
        dinode.uid = uid;
        dinode.gid = gid;
        self.inner.write_inode(self.ino, &dinode)
    }
}
