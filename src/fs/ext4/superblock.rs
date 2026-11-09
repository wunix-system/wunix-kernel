//! Суперблок файловой системы Ext4.
//!
//! Расположен по смещению 1024 байта от начала раздела. Содержит ключевые
//! метаданные файловой системы: размер блока, количество инодов, фичи (EXTENTS, HAS_JOURNAL).

use super::super::vfs::VfsError;

pub const EXT4_SUPERBLOCK_OFFSET: u64 = 1024;
pub const EXT4_MAGIC: u16 = 0xEF53;

// Флаги совместимости (s_feature_compat)
pub const EXT4_FEATURE_COMPAT_DIR_PREALLOC: u32 = 0x0001;
pub const EXT4_FEATURE_COMPAT_IMAGIC_INODES: u32 = 0x0002;
pub const EXT4_FEATURE_COMPAT_HAS_JOURNAL: u32   = 0x0004;
pub const EXT4_FEATURE_COMPAT_EXT_ATTR: u32      = 0x0008;
pub const EXT4_FEATURE_COMPAT_RESIZE_INODE: u32  = 0x0010;
pub const EXT4_FEATURE_COMPAT_DIR_INDEX: u32     = 0x0020;

// Флаги несовместимости (s_feature_incompat)
pub const EXT4_FEATURE_INCOMPAT_COMPRESSION: u32 = 0x0001;
pub const EXT4_FEATURE_INCOMPAT_FILETYPE: u32    = 0x0002;
pub const EXT4_FEATURE_INCOMPAT_RECOVER: u32     = 0x0004;
pub const EXT4_FEATURE_INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
pub const EXT4_FEATURE_INCOMPAT_META_BG: u32     = 0x0010;
pub const EXT4_FEATURE_INCOMPAT_EXTENTS: u32     = 0x0040;
pub const EXT4_FEATURE_INCOMPAT_64BIT: u32       = 0x0080;
pub const EXT4_FEATURE_INCOMPAT_MMP: u32         = 0x0100;
pub const EXT4_FEATURE_INCOMPAT_FLEX_BG: u32     = 0x0200;

#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count: u32,
    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    pub first_data_block: u32,
    pub block_size: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub magic: u16,
    pub state: u16,
    pub first_ino: u32,
    pub inode_size: u16,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub journal_inum: u32,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
}

impl Superblock {
    pub fn parse(buf: &[u8]) -> Result<Self, VfsError> {
        if buf.len() < 1024 {
            return Err(VfsError::IoError);
        }
        let magic = u16::from_le_bytes([buf[56], buf[57]]);
        if magic != EXT4_MAGIC {
            return Err(VfsError::NotFound);
        }

        let log_block_size = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let block_size = 1024u32 << log_block_size;

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&buf[104..120]);

        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&buf[120..136]);

        Ok(Self {
            inodes_count: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            blocks_count: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            free_blocks_count: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            free_inodes_count: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            first_data_block: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            block_size,
            blocks_per_group: u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]),
            inodes_per_group: u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            magic,
            state: u16::from_le_bytes([buf[58], buf[59]]),
            first_ino: u32::from_le_bytes([buf[84], buf[85], buf[86], buf[87]]),
            inode_size: u16::from_le_bytes([buf[88], buf[89]]),
            feature_compat: u32::from_le_bytes([buf[92], buf[93], buf[94], buf[95]]),
            feature_incompat: u32::from_le_bytes([buf[96], buf[97], buf[98], buf[99]]),
            feature_ro_compat: u32::from_le_bytes([buf[100], buf[101], buf[102], buf[103]]),
            journal_inum: u32::from_le_bytes([buf[224], buf[225], buf[226], buf[227]]),
            uuid,
            volume_name,
        })
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        if buf.len() < 1024 {
            return;
        }
        buf[0..4].copy_from_slice(&self.inodes_count.to_le_bytes());
        buf[4..8].copy_from_slice(&self.blocks_count.to_le_bytes());
        buf[12..16].copy_from_slice(&self.free_blocks_count.to_le_bytes());
        buf[16..20].copy_from_slice(&self.free_inodes_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.first_data_block.to_le_bytes());

        let log_block_size: u32 = match self.block_size {
            1024 => 0,
            2048 => 1,
            4096 => 2,
            _ => 2,
        };
        buf[24..28].copy_from_slice(&log_block_size.to_le_bytes());
        buf[32..36].copy_from_slice(&self.blocks_per_group.to_le_bytes());
        buf[40..44].copy_from_slice(&self.inodes_per_group.to_le_bytes());
        buf[56..58].copy_from_slice(&self.magic.to_le_bytes());
        buf[58..60].copy_from_slice(&self.state.to_le_bytes());
        buf[84..88].copy_from_slice(&self.first_ino.to_le_bytes());
        buf[88..90].copy_from_slice(&self.inode_size.to_le_bytes());
        buf[92..96].copy_from_slice(&self.feature_compat.to_le_bytes());
        buf[96..100].copy_from_slice(&self.feature_incompat.to_le_bytes());
        buf[100..104].copy_from_slice(&self.feature_ro_compat.to_le_bytes());
        buf[104..120].copy_from_slice(&self.uuid);
        buf[120..136].copy_from_slice(&self.volume_name);
        buf[224..228].copy_from_slice(&self.journal_inum.to_le_bytes());
    }

    pub fn group_count(&self) -> usize {
        ((self.blocks_count.saturating_sub(self.first_data_block) + self.blocks_per_group - 1)
            / self.blocks_per_group) as usize
    }

    pub fn has_journal(&self) -> bool {
        (self.feature_compat & EXT4_FEATURE_COMPAT_HAS_JOURNAL) != 0 && self.journal_inum != 0
    }

    pub fn has_extents(&self) -> bool {
        (self.feature_incompat & EXT4_FEATURE_INCOMPAT_EXTENTS) != 0
    }
}
