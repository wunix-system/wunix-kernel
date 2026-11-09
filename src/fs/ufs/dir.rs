//! Каталоги UFS2 (struct direct).
//!
//! Запись каталога в BSD UFS2 содержит:
//! - d_ino: номер инода (4 байта)
//! - d_reclen: общая длина записи с выравниванием на 4 байта (2 байта)
//! - d_type: тип файла (1 байт)
//! - d_namlen: длина имени файла (1 байт)
//! - d_name: имя файла

use alloc::string::String;
use alloc::vec::Vec;
use super::super::vfs::{DirEntry, InodeType, VfsError};

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8    = 1;
pub const DT_CHR: u8     = 2;
pub const DT_DIR: u8     = 4;
pub const DT_BLK: u8     = 6;
pub const DT_REG: u8     = 8;
pub const DT_LNK: u8     = 10;
pub const DT_SOCK: u8    = 12;

#[derive(Debug, Clone)]
pub struct UfsDirEntry {
    pub ino: u32,
    pub reclen: u16,
    pub itype_raw: u8,
    pub namlen: u8,
    pub name: String,
    pub offset: usize,
}

impl UfsDirEntry {
    pub fn itype(&self) -> InodeType {
        match self.itype_raw {
            DT_DIR => InodeType::Directory,
            DT_LNK => InodeType::Symlink,
            DT_CHR => InodeType::CharDevice,
            DT_BLK => InodeType::BlockDevice,
            _ => InodeType::File,
        }
    }
}

pub fn parse_dir_entries(block: &[u8]) -> Vec<UfsDirEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;
    let limit = block.len();

    while pos + 8 <= limit {
        let ino = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let reclen = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let itype_raw = block[pos + 6];
        let namlen = block[pos + 7] as usize;

        if reclen < 8 || pos + reclen > limit {
            break;
        }

        if ino != 0 && pos + 8 + namlen <= limit {
            let name_bytes = &block[pos + 8..pos + 8 + namlen];
            if let Ok(name_str) = core::str::from_utf8(name_bytes) {
                entries.push(UfsDirEntry {
                    ino,
                    reclen: reclen as u16,
                    itype_raw,
                    namlen: namlen as u8,
                    name: String::from(name_str),
                    offset: pos,
                });
            }
        }

        pos += reclen;
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
                inode_id: e.ino as u64,
                itype,
            }
        })
        .collect()
}

pub fn find_entry(block: &[u8], target_name: &str) -> Option<UfsDirEntry> {
    parse_dir_entries(block)
        .into_iter()
        .find(|e| e.name == target_name)
}

pub fn init_dir_block(block: &mut [u8], self_ino: u32, parent_ino: u32) {
    block.fill(0);
    // Запись 1: '.'
    block[0..4].copy_from_slice(&self_ino.to_le_bytes());
    block[4..6].copy_from_slice(&12u16.to_le_bytes());
    block[6] = DT_DIR;
    block[7] = 1;
    block[8] = b'.';

    // Запись 2: '..'
    let rest = (block.len() - 12) as u16;
    block[12..16].copy_from_slice(&parent_ino.to_le_bytes());
    block[16..18].copy_from_slice(&rest.to_le_bytes());
    block[18] = DT_DIR;
    block[19] = 2;
    block[20] = b'.';
    block[21] = b'.';
}

pub fn init_empty_dir_block(block: &mut [u8]) {
    let len = block.len() as u16;
    block.fill(0);
    block[0..4].copy_from_slice(&0u32.to_le_bytes());
    block[4..6].copy_from_slice(&len.to_le_bytes());
}

pub fn insert_entry(
    block: &mut [u8],
    ino: u32,
    name: &str,
    itype: InodeType,
) -> Result<(), VfsError> {
    let name_bytes = name.as_bytes();
    let needed_len = ((8 + name_bytes.len() + 3) / 4) * 4;
    let limit = block.len();
    let mut pos = 0;

    let dtype = match itype {
        InodeType::Directory => DT_DIR,
        InodeType::Symlink => DT_LNK,
        InodeType::CharDevice => DT_CHR,
        InodeType::BlockDevice => DT_BLK,
        InodeType::File => DT_REG,
    };

    while pos + 8 <= limit {
        let cur_ino = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let reclen = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let namlen = block[pos + 7] as usize;

        if reclen < 8 || pos + reclen > limit {
            break;
        }

        let actual_used = ((8 + namlen + 3) / 4) * 4;
        let available = reclen.saturating_sub(actual_used);

        if cur_ino == 0 && reclen >= needed_len {
            block[pos..pos + 4].copy_from_slice(&ino.to_le_bytes());
            block[pos + 6] = dtype;
            block[pos + 7] = name_bytes.len() as u8;
            block[pos + 8..pos + 8 + name_bytes.len()].copy_from_slice(name_bytes);
            return Ok(());
        }

        if available >= needed_len {
            block[pos + 4..pos + 6].copy_from_slice(&(actual_used as u16).to_le_bytes());

            let new_pos = pos + actual_used;
            let new_reclen = (reclen - actual_used) as u16;

            block[new_pos..new_pos + 4].copy_from_slice(&ino.to_le_bytes());
            block[new_pos + 4..new_pos + 6].copy_from_slice(&new_reclen.to_le_bytes());
            block[new_pos + 6] = dtype;
            block[new_pos + 7] = name_bytes.len() as u8;
            block[new_pos + 8..new_pos + 8 + name_bytes.len()].copy_from_slice(name_bytes);
            return Ok(());
        }

        pos += reclen;
    }

    Err(VfsError::IoError)
}

pub fn remove_entry(block: &mut [u8], target_name: &str) -> Result<(), VfsError> {
    let limit = block.len();
    let mut pos = 0;
    let mut prev_pos: Option<usize> = None;

    while pos + 8 <= limit {
        let ino = u32::from_le_bytes([block[pos], block[pos + 1], block[pos + 2], block[pos + 3]]);
        let reclen = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
        let namlen = block[pos + 7] as usize;

        if reclen < 8 || pos + reclen > limit {
            break;
        }

        if ino != 0 && pos + 8 + namlen <= limit {
            let name_bytes = &block[pos + 8..pos + 8 + namlen];
            if let Ok(name_str) = core::str::from_utf8(name_bytes) {
                if name_str == target_name {
                    if let Some(prev) = prev_pos {
                        let prev_reclen = u16::from_le_bytes([block[prev + 4], block[prev + 5]]) as usize;
                        let merged_len = (prev_reclen + reclen) as u16;
                        block[prev + 4..prev + 6].copy_from_slice(&merged_len.to_le_bytes());
                    } else {
                        block[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
                    }
                    return Ok(());
                }
            }
        }

        prev_pos = Some(pos);
        pos += reclen;
    }

    Err(VfsError::NotFound)
}
