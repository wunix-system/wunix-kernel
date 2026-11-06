// UFS2 — Unix File System, версия 2 (FreeBSD, NetBSD, OpenBSD).
//
// UFS2 используется как основная ФС в BSD-системах. Суперблок расположен
// по смещению 65536 байт (64 KiB) от начала раздела — это стандарт для UFS2.
// Магическое число 0x19540119 (FS_UFS2_MAGIC).
//
// Текущий статус: структурированная реализация с поддержкой суперблока и базовых метаданных.
// Монтирование через RamInode — для этапа установки достаточно.

use alloc::sync::Arc;

use crate::drivers::BlockDevice;
use crate::fs::ramfs::RamFs;
use crate::fs::vfs::{FileSystem, Inode, VfsError};
use spin::Mutex;

// UFS2 суперблок расположен ровно через 64 KiB от начала раздела.
// 65536 / 512 = 128 секторов — смещение в LBA единицах.
pub const UFS2_SUPER_OFFSET_SECTORS: u64 = 128;
pub const UFS2_MAGIC: i32 = 0x19540119_u32 as i32;

// Суперблок UFS2 велик (1376 байт), читаем 4 сектора (2048 байт) чтобы захватить весь.
// Магическое число находится по смещению 1372 в суперблоке (fs_magic).
// Смещение внутри нашего буфера: 1372 (суперблок начинается прямо с начала сектора 128).
const UFS2_MAGIC_OFFSET: usize = 1372;

fn probe_ufs2(dev: &Arc<Mutex<dyn BlockDevice>>) -> Result<(), VfsError> {
    let mut buf = [0u8; 2048];
    dev.lock()
        .read_sectors(UFS2_SUPER_OFFSET_SECTORS, &mut buf)
        .map_err(|_| VfsError::IoError)?;

    let magic = i32::from_le_bytes([
        buf[UFS2_MAGIC_OFFSET],
        buf[UFS2_MAGIC_OFFSET + 1],
        buf[UFS2_MAGIC_OFFSET + 2],
        buf[UFS2_MAGIC_OFFSET + 3],
    ]);
    if magic != UFS2_MAGIC {
        return Err(VfsError::NotFound);
    }
    Ok(())
}

pub struct UfsFileSystem {
    ram: RamFs,
}

impl UfsFileSystem {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        probe_ufs2(&dev)?;
        Ok(Self { ram: RamFs::new() })
    }
}

impl FileSystem for UfsFileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.ram.root()
    }

    fn name(&self) -> &'static str {
        "ufs"
    }
}

/// Форматирует раздел как UFS2, записывая минимальный суперблок.
///
/// Суперблок UFS2 записывается по смещению 65536 байт (LBA 128) от начала раздела.
/// Мы записываем только магическое число и базовые параметры блочной группы —
/// достаточно для того, чтобы `UfsFileSystem::open()` распознал раздел.
pub fn format_ufs(
    dev: Arc<Mutex<dyn BlockDevice>>,
    part_start_lba: u64,
    total_sectors: u64,
) -> Result<(), VfsError> {
    let mut sb = [0u8; 2048];

    // Базовые поля суперблока UFS2 (структура struct fs из <ufs/ffs/fs.h>):
    //   fs_sblkno (4 байта, смещение 0):  номер LBA суперблока внутри группы блоков
    //   fs_size   (8 байт, смещение 32):  размер ФС в фрагментах
    //   fs_magic  (4 байта, смещение 1372): магическое число UFS2
    let total_frags = (total_sectors / 8) as i64; // 1 фрагмент = 4096 байт = 8 секторов по 512
    sb[0..4].copy_from_slice(&128i32.to_le_bytes());          // fs_sblkno
    sb[32..40].copy_from_slice(&total_frags.to_le_bytes());   // fs_size (int64)
    sb[UFS2_MAGIC_OFFSET..UFS2_MAGIC_OFFSET + 4]
        .copy_from_slice(&UFS2_MAGIC.to_le_bytes());           // fs_magic

    let target_lba = part_start_lba + UFS2_SUPER_OFFSET_SECTORS;
    dev.lock()
        .write_sectors(target_lba, &sb)
        .map_err(|_| VfsError::IoError)?;

    Ok(())
}
