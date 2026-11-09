//! Дисковый индексный узел UFS2 (struct ufs2_dinode).
//!
//! Имеет фиксированный размер 256 байт.
//! Содержит 12 прямых указателей на блоки (di_db) и 3 косвенных указателя (di_ib).

use super::super::vfs::{InodeType, Metadata};

pub const UFS_ROOTINO: u32 = 2;
pub const UFS_NDADDR: usize = 12; // 12 прямых блоков
pub const UFS_NIADDR: usize = 3;  // 3 косвенных блока (одинарный, двойной, тройной)

pub const IFMT: u16   = 0o170000;
pub const IFDIR: u16  = 0o040000;
pub const IFREG: u16  = 0o100000;
pub const IFLNK: u16  = 0o120000;
pub const IFCHR: u16  = 0o020000;
pub const IFBLK: u16  = 0o060000;

#[derive(Debug, Clone)]
pub struct Ufs2Dinode {
    pub mode: u16,
    pub nlink: i16,
    pub uid: u32,
    pub gid: u32,
    pub blksize: u32,
    pub size: u64,
    pub bytes: u64,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub birthtime: i64,
    pub db: [i64; UFS_NDADDR],
    pub ib: [i64; UFS_NIADDR],
}

impl Ufs2Dinode {
    pub fn parse(buf: &[u8]) -> Self {
        let mode = u16::from_le_bytes([buf[0], buf[1]]);
        let nlink = i16::from_le_bytes([buf[2], buf[3]]);
        let uid = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let gid = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let blksize = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let size = u64::from_le_bytes([buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23]]);
        let bytes = u64::from_le_bytes([buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31]]);
        let atime = i64::from_le_bytes([buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39]]);
        let mtime = i64::from_le_bytes([buf[40], buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47]]);
        let ctime = i64::from_le_bytes([buf[48], buf[49], buf[50], buf[51], buf[52], buf[53], buf[54], buf[55]]);
        let birthtime = i64::from_le_bytes([buf[56], buf[57], buf[58], buf[59], buf[60], buf[61], buf[62], buf[63]]);

        let mut db = [0i64; UFS_NDADDR];
        for i in 0..UFS_NDADDR {
            let off = 112 + i * 8;
            db[i] = i64::from_le_bytes([
                buf[off], buf[off + 1], buf[off + 2], buf[off + 3],
                buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7],
            ]);
        }

        let mut ib = [0i64; UFS_NIADDR];
        for i in 0..UFS_NIADDR {
            let off = 112 + UFS_NDADDR * 8 + i * 8;
            ib[i] = i64::from_le_bytes([
                buf[off], buf[off + 1], buf[off + 2], buf[off + 3],
                buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7],
            ]);
        }

        Self {
            mode,
            nlink,
            uid,
            gid,
            blksize,
            size,
            bytes,
            atime,
            mtime,
            ctime,
            birthtime,
            db,
            ib,
        }
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..2].copy_from_slice(&self.mode.to_le_bytes());
        buf[2..4].copy_from_slice(&self.nlink.to_le_bytes());
        buf[4..8].copy_from_slice(&self.uid.to_le_bytes());
        buf[8..12].copy_from_slice(&self.gid.to_le_bytes());
        buf[12..16].copy_from_slice(&self.blksize.to_le_bytes());
        buf[16..24].copy_from_slice(&self.size.to_le_bytes());
        buf[24..32].copy_from_slice(&self.bytes.to_le_bytes());
        buf[32..40].copy_from_slice(&self.atime.to_le_bytes());
        buf[40..48].copy_from_slice(&self.mtime.to_le_bytes());
        buf[48..56].copy_from_slice(&self.ctime.to_le_bytes());
        buf[56..64].copy_from_slice(&self.birthtime.to_le_bytes());

        for i in 0..UFS_NDADDR {
            let off = 112 + i * 8;
            buf[off..off + 8].copy_from_slice(&self.db[i].to_le_bytes());
        }

        for i in 0..UFS_NIADDR {
            let off = 112 + UFS_NDADDR * 8 + i * 8;
            buf[off..off + 8].copy_from_slice(&self.ib[i].to_le_bytes());
        }
    }

    pub fn inode_type(&self) -> InodeType {
        match self.mode & IFMT {
            IFDIR => InodeType::Directory,
            IFLNK => InodeType::Symlink,
            IFCHR => InodeType::CharDevice,
            IFBLK => InodeType::BlockDevice,
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
            InodeType::Directory => IFDIR,
            InodeType::Symlink => IFLNK,
            InodeType::CharDevice => IFCHR,
            InodeType::BlockDevice => IFBLK,
            InodeType::File => IFREG,
        };

        Self {
            mode: type_mask | (mode & 0o7777),
            nlink: if itype == InodeType::Directory { 2 } else { 1 },
            uid,
            gid,
            blksize: 4096,
            size: 0,
            bytes: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            birthtime: 0,
            db: [0i64; UFS_NDADDR],
            ib: [0i64; UFS_NIADDR],
        }
    }
}
