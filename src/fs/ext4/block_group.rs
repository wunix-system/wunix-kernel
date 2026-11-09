//! Дескрипторы групп блоков и битовые карты Ext4.
//!
//! Каждая группа блоков содержит собственную битовую карту блоков, битовую карту инодов
//! и таблицу инодов, что обеспечивает локальность дискового доступа.

use super::super::vfs::VfsError;

#[derive(Debug, Clone, Copy)]
pub struct BlockGroupDesc {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
    pub free_blocks_count: u16,
    pub free_inodes_count: u16,
    pub used_dirs_count: u16,
    pub flags: u16,
}

impl BlockGroupDesc {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            block_bitmap: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            inode_bitmap: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            inode_table: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            free_blocks_count: u16::from_le_bytes([buf[12], buf[13]]),
            free_inodes_count: u16::from_le_bytes([buf[14], buf[15]]),
            used_dirs_count: u16::from_le_bytes([buf[16], buf[17]]),
            flags: u16::from_le_bytes([buf[18], buf[19]]),
        }
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.block_bitmap.to_le_bytes());
        buf[4..8].copy_from_slice(&self.inode_bitmap.to_le_bytes());
        buf[8..12].copy_from_slice(&self.inode_table.to_le_bytes());
        buf[12..14].copy_from_slice(&self.free_blocks_count.to_le_bytes());
        buf[14..16].copy_from_slice(&self.free_inodes_count.to_le_bytes());
        buf[16..18].copy_from_slice(&self.used_dirs_count.to_le_bytes());
        buf[18..20].copy_from_slice(&self.flags.to_le_bytes());
    }
}

/// Поиск и аллокация свободного бита в битовой карте.
/// Возвращает индекс бита (от 0) и устанавливает бит в 1.
pub fn allocate_bit(bitmap: &mut [u8], max_bits: usize) -> Result<usize, VfsError> {
    for (byte_idx, byte) in bitmap.iter_mut().enumerate() {
        if *byte != 0xFF {
            for bit in 0..8 {
                let idx = byte_idx * 8 + bit;
                if idx >= max_bits {
                    return Err(VfsError::IoError);
                }
                if (*byte & (1 << bit)) == 0 {
                    *byte |= 1 << bit;
                    return Ok(idx);
                }
            }
        }
    }
    Err(VfsError::IoError)
}

/// Освобождение бита в битовой карте (сброс в 0).
pub fn free_bit(bitmap: &mut [u8], bit_idx: usize) {
    let byte_idx = bit_idx / 8;
    let bit = bit_idx % 8;
    if byte_idx < bitmap.len() {
        bitmap[byte_idx] &= !(1 << bit);
    }
}

/// Проверка, установлен ли бит в битовой карте.
pub fn is_bit_set(bitmap: &[u8], bit_idx: usize) -> bool {
    let byte_idx = bit_idx / 8;
    let bit = bit_idx % 8;
    if byte_idx < bitmap.len() {
        (bitmap[byte_idx] & (1 << bit)) != 0
    } else {
        false
    }
}
