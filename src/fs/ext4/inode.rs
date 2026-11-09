//! Дисковый индексный узел (Inode) Ext4.
//!
//! Стандартный размер инода в Ext4 — 256 байт.
//! Содержит тип файла, права доступа POSIX, размер, временные метки
//! и 60-байтный буфер i_block, в котором размещается корень дерева экстентов.

use super::super::vfs::{InodeType, Metadata};

pub const EXT4_INODE_ROOT_INO: u32 = 2;
pub const EXT4_INODE_JOURNAL_INO: u32 = 8;

pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

// POSIX i_mode маски
pub const S_IFMT: u16   = 0o170000;
pub const S_IFIFO: u16  = 0o010000;
pub const S_IFCHR: u16  = 0o020000;
pub const S_IFDIR: u16  = 0o040000;
pub const S_IFBLK: u16  = 0o060000;
pub const S_IFREG: u16  = 0o100000;
pub const S_IFLNK: u16  = 0o120000;
pub const S_IFSOCK: u16 = 0o140000;

#[derive(Debug, Clone)]
pub struct DiskInode {
    pub mode: u16,
    pub uid: u32,
    pub size: u64,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub gid: u32,
    pub links_count: u16,
    pub blocks_512: u64,
    pub flags: u32,
    pub block: [u8; 60],
}

impl DiskInode {
    pub fn parse(buf: &[u8]) -> Self {
        let mode = u16::from_le_bytes([buf[0], buf[1]]);
        let uid_lo = u16::from_le_bytes([buf[2], buf[3]]) as u32;
        let size_lo = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as u64;
        let atime = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let ctime = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let mtime = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let gid_lo = u16::from_le_bytes([buf[24], buf[25]]) as u32;
        let links_count = u16::from_le_bytes([buf[26], buf[27]]);
        let blocks_lo = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]) as u64;
        let flags = u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]);

        let mut block = [0u8; 60];
        block.copy_from_slice(&buf[40..100]);

        // Читаем старшие части для 64-битного размера и 32-битных UID/GID (при размере инода >= 128)
        let size_hi = if buf.len() >= 112 && (mode & S_IFMT) == S_IFREG {
            u32::from_le_bytes([buf[108], buf[109], buf[110], buf[111]]) as u64
        } else {
            0
        };
        let size = (size_hi << 32) | size_lo;

        let uid_hi = if buf.len() >= 122 {
            u16::from_le_bytes([buf[120], buf[121]]) as u32
        } else {
            0
        };
        let gid_hi = if buf.len() >= 124 {
            u16::from_le_bytes([buf[122], buf[123]]) as u32
        } else {
            0
        };

        let uid = (uid_hi << 16) | uid_lo;
        let gid = (gid_hi << 16) | gid_lo;

        Self {
            mode,
            uid,
            size,
            atime,
            ctime,
            mtime,
            gid,
            links_count,
            blocks_512: blocks_lo,
            flags,
            block,
        }
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..2].copy_from_slice(&self.mode.to_le_bytes());
        buf[2..4].copy_from_slice(&(self.uid as u16).to_le_bytes());
        buf[4..8].copy_from_slice(&(self.size as u32).to_le_bytes());
        buf[8..12].copy_from_slice(&self.atime.to_le_bytes());
        buf[12..16].copy_from_slice(&self.ctime.to_le_bytes());
        buf[16..20].copy_from_slice(&self.mtime.to_le_bytes());
        buf[24..26].copy_from_slice(&(self.gid as u16).to_le_bytes());
        buf[26..28].copy_from_slice(&self.links_count.to_le_bytes());
        buf[28..32].copy_from_slice(&(self.blocks_512 as u32).to_le_bytes());
        buf[32..36].copy_from_slice(&self.flags.to_le_bytes());
        buf[40..100].copy_from_slice(&self.block);

        if buf.len() >= 128 {
            if (self.mode & S_IFMT) == S_IFREG {
                let size_hi = (self.size >> 32) as u32;
                buf[108..112].copy_from_slice(&size_hi.to_le_bytes());
            }
            let uid_hi = (self.uid >> 16) as u16;
            let gid_hi = (self.gid >> 16) as u16;
            buf[120..122].copy_from_slice(&uid_hi.to_le_bytes());
            buf[122..124].copy_from_slice(&gid_hi.to_le_bytes());
        }
    }

    pub fn inode_type(&self) -> InodeType {
        match self.mode & S_IFMT {
            S_IFDIR => InodeType::Directory,
            S_IFLNK => InodeType::Symlink,
            S_IFCHR => InodeType::CharDevice,
            S_IFBLK => InodeType::BlockDevice,
            _ => InodeType::File,
        }
    }

    pub fn to_metadata(&self, inode_id: u64) -> Metadata {
        Metadata {
            itype: self.inode_type(),
            size: self.size as usize,
            inode_id,
            permissions: self.mode & 0o7777,
            uid: self.uid,
            gid: self.gid,
        }
    }

    pub fn new_empty(itype: InodeType, mode: u16, uid: u32, gid: u32) -> Self {
        let type_mask = match itype {
            InodeType::Directory => S_IFDIR,
            InodeType::Symlink => S_IFLNK,
            InodeType::CharDevice => S_IFCHR,
            InodeType::BlockDevice => S_IFBLK,
            InodeType::File => S_IFREG,
        };

        let mut block = [0u8; 60];
        // Инициализируем заголовок дерева экстентов
        block[0..2].copy_from_slice(&0xF30Au16.to_le_bytes()); // eh_magic
        block[2..4].copy_from_slice(&0u16.to_le_bytes());      // eh_entries = 0
        block[4..6].copy_from_slice(&4u16.to_le_bytes());      // eh_max = 4
        block[6..8].copy_from_slice(&0u16.to_le_bytes());      // eh_depth = 0 (leaf)
        block[8..12].copy_from_slice(&0u32.to_le_bytes());     // eh_generation = 0

        Self {
            mode: type_mask | (mode & 0o7777),
            uid,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            gid,
            links_count: if itype == InodeType::Directory { 2 } else { 1 },
            blocks_512: 0,
            flags: EXT4_EXTENTS_FL,
            block,
        }
    }
}
