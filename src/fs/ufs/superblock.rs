//! Суперблок файловой системы UFS2 (Unix File System v2).
//!
//! Стандартное смещение суперблока UFS2 составляет 65536 байт (128 секторов по 512 байт, LBA 128).
//! Магическое число 0x19540119 (FS_UFS2_MAGIC). Размер структуры fs: 1376 байт.

use super::super::vfs::VfsError;

pub const UFS2_SUPERBLOCK_SECTOR: u64 = 128;
pub const UFS2_SUPERBLOCK_OFFSET: u64 = 65536;
pub const UFS2_MAGIC: u32 = 0x19540119;

// Флаги суперблока UFS2
pub const FS_UNCLEAN: i32    = 0x0001;
pub const FS_DOSOFTDEP: i32  = 0x0020;
pub const FS_SUJ: i32        = 0x0040; // Soft Updates Journaling

#[derive(Debug, Clone, Copy)]
pub struct Ufs2Superblock {
    pub sblkno: i32,
    pub cblkno: i32,
    pub iblkno: i32,
    pub dblkno: i32,
    pub bshift: i32,
    pub fshift: i32,
    pub bsize: i32,
    pub fsize: i32,
    pub frag: i32,
    pub size: i64,
    pub dsize: i64,
    pub ncg: i32,
    pub fpg: i32,
    pub ipg: i32,
    pub magic: u32,
    pub flags: i32,
    pub sujfree: i64,
}

impl Ufs2Superblock {
    pub fn parse(buf: &[u8]) -> Result<Self, VfsError> {
        if buf.len() < 1376 {
            return Err(VfsError::IoError);
        }
        let magic = u32::from_le_bytes([buf[1372], buf[1373], buf[1374], buf[1375]]);
        if magic != UFS2_MAGIC {
            return Err(VfsError::NotFound);
        }

        Ok(Self {
            sblkno: i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            cblkno: i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            iblkno: i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            dblkno: i32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            bshift: i32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]),
            fshift: i32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            bsize: i32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]),
            fsize: i32::from_le_bytes([buf[52], buf[53], buf[54], buf[55]]),
            frag: i32::from_le_bytes([buf[56], buf[57], buf[58], buf[59]]),
            size: i64::from_le_bytes([buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39]]),
            dsize: i64::from_le_bytes([buf[64], buf[65], buf[66], buf[67], buf[68], buf[69], buf[70], buf[71]]),
            ncg: i32::from_le_bytes([buf[196], buf[197], buf[198], buf[199]]),
            fpg: i32::from_le_bytes([buf[204], buf[205], buf[206], buf[207]]),
            ipg: i32::from_le_bytes([buf[208], buf[209], buf[210], buf[211]]),
            magic,
            flags: i32::from_le_bytes([buf[1368], buf[1369], buf[1370], buf[1371]]),
            sujfree: i64::from_le_bytes([buf[1360], buf[1361], buf[1362], buf[1363], buf[1364], buf[1365], buf[1366], buf[1367]]),
        })
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        if buf.len() < 1376 {
            return;
        }
        buf[0..4].copy_from_slice(&self.sblkno.to_le_bytes());
        buf[4..8].copy_from_slice(&self.cblkno.to_le_bytes());
        buf[8..12].copy_from_slice(&self.iblkno.to_le_bytes());
        buf[12..16].copy_from_slice(&self.dblkno.to_le_bytes());
        buf[32..40].copy_from_slice(&self.size.to_le_bytes());
        buf[48..52].copy_from_slice(&self.bsize.to_le_bytes());
        buf[52..56].copy_from_slice(&self.fsize.to_le_bytes());
        buf[56..60].copy_from_slice(&self.frag.to_le_bytes());
        buf[64..72].copy_from_slice(&self.dsize.to_le_bytes());
        buf[196..200].copy_from_slice(&self.ncg.to_le_bytes());
        buf[204..208].copy_from_slice(&self.fpg.to_le_bytes());
        buf[208..212].copy_from_slice(&self.ipg.to_le_bytes());
        buf[1360..1368].copy_from_slice(&self.sujfree.to_le_bytes());
        buf[1368..1372].copy_from_slice(&self.flags.to_le_bytes());
        buf[1372..1376].copy_from_slice(&self.magic.to_le_bytes());
    }

    pub fn has_suj(&self) -> bool {
        (self.flags & FS_SUJ) != 0
    }
}
