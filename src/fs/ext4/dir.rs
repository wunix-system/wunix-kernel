//! Каталоги Ext4 (Directory Entries).
//!
//! Ext4 использует формат ext4_dir_entry_2 с явным полем file_type,
//! что устраняет необходимость считывать инод каждого дочернего файла при readdir.

use alloc::string::String;
use alloc::vec::Vec;
use super::super::vfs::{DirEntry, InodeType, VfsError};

pub const EXT4_FT_UNKNOWN: u8  = 0;
pub const EXT4_FT_REG_FILE: u8 = 1;
pub const EXT4_FT_DIR: u8      = 2;
pub const EXT4_FT_CHRDEV: u8   = 3;
pub const EXT4_FT_BLKDEV: u8   = 4;
pub const EXT4_FT_FIFO: u8     = 5;
pub const EXT4_FT_SOCK: u8     = 6;
pub const EXT4_FT_SYMLINK: u8  = 7;

#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    pub inode: u32,
    pub rec_len: u16,
    pub name_len: u8,
    pub file_type: u8,
    pub name: String,
    pub offset: usize,
}

impl Ext4DirEntry {
    pub fn itype(&self) -> InodeType {
        match self.file_type {
            EXT4_FT_DIR => InodeType::Directory,
            EXT4_FT_SYMLINK => InodeType::Symlink,
            EXT4_FT_CHRDEV => InodeType::CharDevice,
            EXT4_FT_BLKDEV => InodeType::BlockDevice,
            _ => InodeType::File,
        }
    }
}

pub fn parse_dir_entries(block: &[u8]) -> Vec<Ext4DirEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;
    let limit = block.len();

    while pos + 8 <= limit {
        let inode = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let rec_len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let name_len = block[pos + 6] as usize;
        let file_type = block[pos + 7];

        if rec_len < 8 || pos + rec_len > limit {
            break;
        }

        if inode != 0 && pos + 8 + name_len <= limit {
            let name_bytes = &block[pos + 8..pos + 8 + name_len];
            if let Ok(name_str) = core::str::from_utf8(name_bytes) {
                entries.push(Ext4DirEntry {
                    inode,
                    rec_len: rec_len as u16,
                    name_len: name_len as u8,
                    file_type,
                    name: String::from(name_str),
                    offset: pos,
                });
            }
        }

        pos += rec_len;
    }

    entries
}

pub fn to_vfs_entries(block: &[u8]) -> Vec<DirEntry> {
    parse_dir_entries(block)
        .into_iter()
        .filter(|e| e.name != "." && e.name != "..")
        .map(|e| {
            let itype = e.itype();
            DirEntry {
                name: e.name,
                inode_id: e.inode as u64,
                itype,
            }
        })
        .collect()
}

pub fn find_entry(block: &[u8], target_name: &str) -> Option<Ext4DirEntry> {
    parse_dir_entries(block)
        .into_iter()
        .find(|e| e.name == target_name)
}

pub fn init_dir_block(block: &mut [u8], self_ino: u32, parent_ino: u32) {
    block.fill(0);
    // Запись 1: '.'
    block[0..4].copy_from_slice(&self_ino.to_le_bytes());
    block[4..6].copy_from_slice(&12u16.to_le_bytes());
    block[6] = 1;
    block[7] = EXT4_FT_DIR;
    block[8] = b'.';

    // Запись 2: '..'
    let rest = (block.len() - 12) as u16;
    block[12..16].copy_from_slice(&parent_ino.to_le_bytes());
    block[16..18].copy_from_slice(&rest.to_le_bytes());
    block[18] = 2;
    block[19] = EXT4_FT_DIR;
    block[20] = b'.';
    block[21] = b'.';
}

pub fn init_empty_dir_block(block: &mut [u8]) {
    let len = block.len() as u16;
    block.fill(0);
    block[0..4].copy_from_slice(&0u32.to_le_bytes());
    block[4..6].copy_from_slice(&len.to_le_bytes());
}

/// Добавление записи каталога в блок.
/// Ищет свободное пространство в конце последней записи и встраивает новую запись.
pub fn insert_entry(
    block: &mut [u8],
    inode: u32,
    name: &str,
    itype: InodeType,
) -> Result<(), VfsError> {
    let name_bytes = name.as_bytes();
    let needed_len = ((8 + name_bytes.len() + 3) / 4) * 4;
    let limit = block.len();
    let mut pos = 0;

    let file_type = match itype {
        InodeType::Directory => EXT4_FT_DIR,
        InodeType::Symlink => EXT4_FT_SYMLINK,
        InodeType::CharDevice => EXT4_FT_CHRDEV,
        InodeType::BlockDevice => EXT4_FT_BLKDEV,
        InodeType::File => EXT4_FT_REG_FILE,
    };

    while pos + 8 <= limit {
        let cur_inode = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let rec_len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let name_len = block[pos + 6] as usize;

        if rec_len < 8 || pos + rec_len > limit {
            break;
        }

        let actual_used = ((8 + name_len + 3) / 4) * 4;
        let available = rec_len.saturating_sub(actual_used);

        if cur_inode == 0 && rec_len >= needed_len {
            // Занимаем удаленную/пустую запись
            block[pos..pos + 4].copy_from_slice(&inode.to_le_bytes());
            block[pos + 6] = name_bytes.len() as u8;
            block[pos + 7] = file_type;
            block[pos + 8..pos + 8 + name_bytes.len()].copy_from_slice(name_bytes);
            return Ok(());
        }

        if available >= needed_len {
            // Усекаем текущую запись до actual_used и вставляем новую
            block[pos + 4..pos + 6].copy_from_slice(&(actual_used as u16).to_le_bytes());

            let new_pos = pos + actual_used;
            let new_rec_len = (rec_len - actual_used) as u16;

            block[new_pos..new_pos + 4].copy_from_slice(&inode.to_le_bytes());
            block[new_pos + 4..new_pos + 6].copy_from_slice(&new_rec_len.to_le_bytes());
            block[new_pos + 6] = name_bytes.len() as u8;
            block[new_pos + 7] = file_type;
            block[new_pos + 8..new_pos + 8 + name_bytes.len()].copy_from_slice(name_bytes);
            return Ok(());
        }

        pos += rec_len;
    }

    Err(VfsError::IoError)
}

/// Удаление записи каталога по имени.
/// Сбрасывает inode в 0 и объединяет свободное пространство с предыдущей записью.
pub fn remove_entry(block: &mut [u8], target_name: &str) -> Result<(), VfsError> {
    let limit = block.len();
    let mut pos = 0;
    let mut prev_pos: Option<usize> = None;

    while pos + 8 <= limit {
        let inode = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let rec_len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let name_len = block[pos + 6] as usize;

        if rec_len < 8 || pos + rec_len > limit {
            break;
        }

        if inode != 0 && pos + 8 + name_len <= limit {
            let name_bytes = &block[pos + 8..pos + 8 + name_len];
            if let Ok(name_str) = core::str::from_utf8(name_bytes) {
                if name_str == target_name {
                    if let Some(prev) = prev_pos {
                        let prev_rec_len = u16::from_le_bytes([block[prev + 4], block[prev + 5]]) as usize;
                        let merged_len = (prev_rec_len + rec_len) as u16;
                        block[prev + 4..prev + 6].copy_from_slice(&merged_len.to_le_bytes());
                    } else {
                        block[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
                    }
                    return Ok(());
                }
            }
        }

        prev_pos = Some(pos);
        pos += rec_len;
    }

    Err(VfsError::NotFound)
}
