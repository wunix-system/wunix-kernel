//! Дерево экстентов Ext4 (Extent Tree).
//!
//! Экстенты заменяют традиционную схему косвенных блоков Ext2/3,
//! позволяя адресовать непрерывные диапазоны до 32768 блоков одной 12-байтной записью.

use super::super::vfs::VfsError;

pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

#[derive(Debug, Clone, Copy)]
pub struct ExtentHeader {
    pub magic: u16,
    pub entries: u16,
    pub max: u16,
    pub depth: u16,
    pub generation: u32,
}

impl ExtentHeader {
    pub fn parse(buf: &[u8]) -> Result<Self, VfsError> {
        if buf.len() < 12 {
            return Err(VfsError::IoError);
        }
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != EXT4_EXT_MAGIC {
            return Err(VfsError::IoError);
        }
        Ok(Self {
            magic,
            entries: u16::from_le_bytes([buf[2], buf[3]]),
            max: u16::from_le_bytes([buf[4], buf[5]]),
            depth: u16::from_le_bytes([buf[6], buf[7]]),
            generation: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        })
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..2].copy_from_slice(&self.magic.to_le_bytes());
        buf[2..4].copy_from_slice(&self.entries.to_le_bytes());
        buf[4..6].copy_from_slice(&self.max.to_le_bytes());
        buf[6..8].copy_from_slice(&self.depth.to_le_bytes());
        buf[8..12].copy_from_slice(&self.generation.to_le_bytes());
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExtentLeaf {
    pub block: u32,
    pub len: u16,
    pub start_hi: u16,
    pub start_lo: u32,
}

impl ExtentLeaf {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            block: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            len: u16::from_le_bytes([buf[4], buf[5]]),
            start_hi: u16::from_le_bytes([buf[6], buf[7]]),
            start_lo: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        }
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.block.to_le_bytes());
        buf[4..6].copy_from_slice(&self.len.to_le_bytes());
        buf[6..8].copy_from_slice(&self.start_hi.to_le_bytes());
        buf[8..12].copy_from_slice(&self.start_lo.to_le_bytes());
    }

    pub fn physical_start(&self) -> u64 {
        ((self.start_hi as u64) << 32) | (self.start_lo as u64)
    }

    pub fn contains(&self, logical_block: u32) -> bool {
        let clean_len = (self.len & 0x7FFF) as u32;
        logical_block >= self.block && logical_block < self.block + clean_len
    }

    pub fn map_block(&self, logical_block: u32) -> u64 {
        let offset = (logical_block - self.block) as u64;
        self.physical_start() + offset
    }
}

/// Трансляция логического блока файла в физический блок раздела по дереву экстентов в i_block.
pub fn lookup_logical_block(i_block: &[u8], logical_block: u32) -> Result<Option<u64>, VfsError> {
    let header = ExtentHeader::parse(i_block)?;
    if header.depth == 0 {
        // Листовой уровень непосредственно в иноде
        for i in 0..header.entries as usize {
            let offset = 12 + i * 12;
            if offset + 12 > i_block.len() {
                break;
            }
            let extent = ExtentLeaf::parse(&i_block[offset..offset + 12]);
            if extent.contains(logical_block) {
                return Ok(Some(extent.map_block(logical_block)));
            }
        }
        Ok(None)
    } else {
        // Одноуровневое индексное дерево
        for i in 0..header.entries as usize {
            let offset = 12 + i * 12;
            if offset + 12 > i_block.len() {
                break;
            }
            let idx_block = u32::from_le_bytes([i_block[offset], i_block[offset + 1], i_block[offset + 2], i_block[offset + 3]]);
            let leaf_lo = u32::from_le_bytes([i_block[offset + 4], i_block[offset + 5], i_block[offset + 6], i_block[offset + 7]]);
            let leaf_hi = u16::from_le_bytes([i_block[offset + 8], i_block[offset + 9]]);
            let leaf_phys = ((leaf_hi as u64) << 32) | (leaf_lo as u64);
            if logical_block >= idx_block {
                return Ok(Some(leaf_phys));
            }
        }
        Ok(None)
    }
}

/// Добавление экстента в корень дерева (внутри 60-байтного буфера инода).
pub fn append_extent(i_block: &mut [u8], logical_block: u32, physical_block: u64, count: u16) -> Result<(), VfsError> {
    let mut header = ExtentHeader::parse(i_block)?;
    if header.depth != 0 {
        return Err(VfsError::NotSupported);
    }

    // Проверяем, можно ли расширить последний существующий экстент
    if header.entries > 0 {
        let last_idx = (header.entries - 1) as usize;
        let offset = 12 + last_idx * 12;
        let mut last_ext = ExtentLeaf::parse(&i_block[offset..offset + 12]);
        let clean_len = last_ext.len & 0x7FFF;
        let expected_logical = last_ext.block + clean_len as u32;
        let expected_phys = last_ext.physical_start() + clean_len as u64;

        if logical_block == expected_logical && physical_block == expected_phys && ((clean_len as u32) + (count as u32) <= 32768) {
            last_ext.len += count;
            last_ext.write_to_slice(&mut i_block[offset..offset + 12]);
            return Ok(());
        }
    }

    // Если объединить нельзя, добавляем новую запись в листовой узел
    if header.entries >= header.max {
        return Err(VfsError::IoError); // Для базового ядра поддерживаем до 4 экстентов в иноде
    }

    let offset = 12 + (header.entries as usize) * 12;
    let new_ext = ExtentLeaf {
        block: logical_block,
        len: count,
        start_hi: (physical_block >> 32) as u16,
        start_lo: (physical_block & 0xFFFF_FFFF) as u32,
    };
    new_ext.write_to_slice(&mut i_block[offset..offset + 12]);

    header.entries += 1;
    header.write_to_slice(&mut i_block[0..12]);
    Ok(())
}
