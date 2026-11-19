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
use self::block_group::{allocate_bit, free_bit, BlockGroupDesc};
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
                if self.read_block(desc.block_bitmap as u64, &mut bitmap).is_err() {
                    continue;
                }

                let bit = match allocate_bit(&mut bitmap, sb.blocks_per_group as usize) {
                    Ok(b) => b,
                    Err(_) => {
                        desc.free_blocks_count = 0;
                        continue;
                    }
                };
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
                if self.read_block(desc.inode_bitmap as u64, &mut bitmap).is_err() {
                    continue;
                }

                let bit = match allocate_bit(&mut bitmap, sb.inodes_per_group as usize) {
                    Ok(b) => b,
                    Err(_) => {
                        desc.free_inodes_count = 0;
                        continue;
                    }
                };
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

    pub fn free_inode(&self, ino: u32) -> Result<(), VfsError> {
        if ino == 0 {
            return Ok(());
        }
        let mut groups = self.group_descs.lock();
        let mut sb = self.sb.lock();
        let grp_idx = ((ino - 1) / sb.inodes_per_group) as usize;
        let bit = ((ino - 1) % sb.inodes_per_group) as usize;
        if grp_idx >= groups.len() {
            return Err(VfsError::IoError);
        }
        let desc = &mut groups[grp_idx];
        let mut bitmap = vec![0u8; sb.block_size as usize];
        self.read_block(desc.inode_bitmap as u64, &mut bitmap)?;
        free_bit(&mut bitmap, bit);
        self.write_block(desc.inode_bitmap as u64, &bitmap)?;
        desc.free_inodes_count = desc.free_inodes_count.saturating_add(1);
        sb.free_inodes_count = sb.free_inodes_count.saturating_add(1);
        Ok(())
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

impl Ext4Inode {
    fn insert_child_entry(&self, ino: u32, name: &str, itype: InodeType) -> Result<(), VfsError> {
        let mut dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
        let block_size = self.inner.sb.lock().block_size as usize;
        let mut inserted = false;
        let mut temp_block = vec![0u8; block_size];
        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;

        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if insert_entry(&mut temp_block, ino, name, itype).is_ok() {
                    self.inner.write_block(phys, &temp_block)?;
                    inserted = true;
                    break;
                }
            }
        }

        if !inserted {
            let new_dir_block = self.inner.allocate_block()?;
            let logical_idx = total_blocks as u32;
            append_extent(&mut dinode.block, logical_idx, new_dir_block, 1)?;
            dinode.size += block_size as u64;
            dinode.blocks_512 += (block_size / 512) as u64;

            dir::init_empty_dir_block(&mut temp_block);
            insert_entry(&mut temp_block, ino, name, itype)?;
            self.inner.write_block(new_dir_block, &temp_block)?;
            self.inner.write_inode(self.ino, &dinode)?;
        }

        *self.dinode.lock() = dinode;
        Ok(())
    }

    fn remove_child_entry(&self, name: &str) -> Result<u32, VfsError> {
        let dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
        let block_size = self.inner.sb.lock().block_size as usize;
        let mut temp_block = vec![0u8; block_size];
        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;

        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if let Ok(unlinked_ino) = remove_entry(&mut temp_block, name) {
                    self.inner.write_block(phys, &temp_block)?;
                    return Ok(unlinked_ino);
                }
            }
        }
        Err(VfsError::NotFound)
    }
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
        let dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
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
        let block_size = self.inner.sb.lock().block_size as usize;
        {
            let dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
            if dinode.inode_type() != InodeType::Directory {
                return Err(VfsError::NotADirectory);
            }

            let mut temp_block = vec![0u8; block_size];
            let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;

            // Предотвращаем дублирование записей в каталоге при повторном создании
            for b in 0..total_blocks {
                if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                    self.inner.read_block(phys, &mut temp_block)?;
                    if find_entry(&temp_block, name).is_some() {
                        return Err(VfsError::AlreadyExists);
                    }
                }
            }
        }

        let new_ino = self.inner.allocate_inode(itype)?;

        let (perms, uid, gid) = match itype {
            InodeType::Directory => (0o755, 0, 0),
            InodeType::Symlink => (0o777, 0, 0),
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

        self.insert_child_entry(new_ino, name, itype)?;

        Ok(Arc::new(Ext4Inode {
            inner: self.inner.clone(),
            ino: new_ino,
            dinode: Mutex::new(new_dinode),
        }))
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let unlinked_ino = self.remove_child_entry(name)?;
        if let Ok(mut target_dinode) = self.inner.read_inode(unlinked_ino) {
            target_dinode.links_count = target_dinode.links_count.saturating_sub(1);
            if target_dinode.links_count == 0 {
                let _ = self.inner.free_inode(unlinked_ino);
            } else {
                let _ = self.inner.write_inode(unlinked_ino, &target_dinode);
            }
        } else {
            let _ = self.inner.free_inode(unlinked_ino);
        }
        Ok(())
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
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

    fn truncate(&self, len: usize) -> Result<(), VfsError> {
        let mut dinode = self.dinode.lock();
        if dinode.inode_type() == InodeType::Directory {
            return Err(VfsError::IsADirectory);
        }
        dinode.size = len as u64;
        self.inner.write_inode(self.ino, &dinode)
    }

    fn rename(
        &self,
        old_name: &str,
        new_parent: &Arc<dyn Inode>,
        new_name: &str,
    ) -> Result<(), VfsError> {
        let new_ext4 = new_parent
            .as_any()
            .and_then(|a| a.downcast_ref::<Ext4Inode>())
            .ok_or(VfsError::NotSupported)?;

        let dinode = self.inner.read_inode(self.ino).unwrap_or_else(|_| self.dinode.lock().clone());
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }
        let block_size = self.inner.sb.lock().block_size as usize;
        let mut temp_block = vec![0u8; block_size];
        let total_blocks = ((dinode.size as usize) + block_size - 1) / block_size;
        let mut target_ino = None;
        let mut target_itype = InodeType::File;

        for b in 0..total_blocks {
            if let Some(phys) = lookup_logical_block(&dinode.block, b as u32)? {
                self.inner.read_block(phys, &mut temp_block)?;
                if let Some(entry) = find_entry(&temp_block, old_name) {
                    target_ino = Some(entry.inode);
                    target_itype = entry.itype();
                    break;
                }
            }
        }

        let target_ino = target_ino.ok_or(VfsError::NotFound)?;

        if new_ext4.lookup(new_name).is_ok() {
            let _ = new_ext4.unlink(new_name);
        }

        // Перемещаем запись: удаляем старую запись каталога БЕЗ освобождения самого инода
        self.remove_child_entry(old_name)?;
        new_ext4.insert_child_entry(target_ino, new_name, target_itype)?;

        // Если переносится поддиректория в другого родителя, обновляем ее запись '..'
        if target_itype == InodeType::Directory && self.ino != new_ext4.ino {
            if let Ok(target_dinode) = self.inner.read_inode(target_ino) {
                if let Ok(Some(first_block)) = lookup_logical_block(&target_dinode.block, 0) {
                    let mut dir_block = vec![0u8; block_size];
                    if self.inner.read_block(first_block, &mut dir_block).is_ok() {
                        if dir_block.len() >= 24 && dir_block[18] == 2 && &dir_block[20..22] == b".." {
                            dir_block[12..16].copy_from_slice(&new_ext4.ino.to_le_bytes());
                            let _ = self.inner.write_block(first_block, &dir_block);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}
