//! Группы цилиндров UFS2 (Cylinder Groups).
//!
//! Каждая группа цилиндров содержит заголовок struct cg (магическое число 0x090255),
//! битовую карту свободных фрагментов/блоков и битовую карту инодов.

use super::super::vfs::VfsError;

pub const CG_MAGIC: i32 = 0x090255;

#[derive(Debug, Clone, Copy)]
pub struct CylinderGroup {
    pub cgx: i32,
    pub ndblk: i32,
    pub niblk: i32,
    pub nfree: i32,
    pub nifree: i32,
    pub iusedoff: i32,
    pub freeoff: i32,
}

impl CylinderGroup {
    pub fn parse(buf: &[u8]) -> Result<Self, VfsError> {
        if buf.len() < 128 {
            return Err(VfsError::IoError);
        }
        let magic = i32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        if magic != CG_MAGIC {
            return Err(VfsError::NotFound);
        }

        Ok(Self {
            cgx: i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            ndblk: i32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            niblk: i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            nfree: i32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            nifree: i32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]),
            iusedoff: i32::from_le_bytes([buf[64], buf[65], buf[66], buf[67]]),
            freeoff: i32::from_le_bytes([buf[68], buf[69], buf[70], buf[71]]),
        })
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[4..8].copy_from_slice(&self.cgx.to_le_bytes());
        buf[16..20].copy_from_slice(&self.ndblk.to_le_bytes());
        buf[20..24].copy_from_slice(&self.niblk.to_le_bytes());
        buf[24..28].copy_from_slice(&CG_MAGIC.to_le_bytes());
        buf[40..44].copy_from_slice(&self.nfree.to_le_bytes());
        buf[44..48].copy_from_slice(&self.nifree.to_le_bytes());
        buf[64..68].copy_from_slice(&self.iusedoff.to_le_bytes());
        buf[68..72].copy_from_slice(&self.freeoff.to_le_bytes());
    }
}

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

pub fn free_bit(bitmap: &mut [u8], bit_idx: usize) {
    let byte_idx = bit_idx / 8;
    let bit = bit_idx % 8;
    if byte_idx < bitmap.len() {
        bitmap[byte_idx] &= !(1 << bit);
    }
}
