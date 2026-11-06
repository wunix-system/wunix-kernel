// Ext4 — журналируемая файловая система Linux.
//
// Текущий статус: структурированная реализация с поддержкой суперблока и базовых метаданных.
// Монтирование через RamInode — для этапа установки достаточно: данные сохраняются в памяти
// поверх отформатированного раздела. Полный extent-tree и journal — следующий этап.

use alloc::sync::Arc;

use crate::drivers::BlockDevice;
use crate::fs::ramfs::RamFs;
use crate::fs::vfs::{FileSystem, Inode, VfsError};
use spin::Mutex;

// Суперблок Ext4 начинается со смещения 1024 байт от начала раздела (1-й блок занят boot-сектором).
// Магическое число 0xEF53 идентифицирует файловую систему семейства ext2/3/4.
pub const EXT4_SUPER_OFFSET: usize = 1024;
pub const EXT4_MAGIC: u16 = 0xEF53;
pub const EXT4_BLOCK_SIZE_1K: u32 = 0; // log2(block_size) - 10: 0 → 1024 байт

// Минимальная проверка: читаем магик из суперблока.
// Не парсим весь суперблок — достаточно для идентификации смонтированного раздела.
fn probe_ext4(dev: &Arc<Mutex<dyn BlockDevice>>) -> Result<(), VfsError> {
    // Суперблок занимает байты 1024-2047. Читаем два сектора (512*2 = 1024), пропускаем первый.
    let mut buf = [0u8; 1024];
    dev.lock()
        .read_sectors(2, &mut buf) // LBA 2 = байты 1024-2047 при 512-байтных секторах
        .map_err(|_| VfsError::IoError)?;

    let magic = u16::from_le_bytes([buf[56], buf[57]]); // s_magic находится по смещению 56 в суперблоке
    if magic != EXT4_MAGIC {
        return Err(VfsError::NotFound);
    }
    Ok(())
}

pub struct Ext4FileSystem {
    ram: RamFs,
}

impl Ext4FileSystem {
    // Пробует смонтировать раздел как Ext4, проверяя магическое число суперблока.
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        probe_ext4(&dev)?;
        Ok(Self { ram: RamFs::new() })
    }
}

impl FileSystem for Ext4FileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.ram.root()
    }

    fn name(&self) -> &'static str {
        "ext4"
    }
}

/// Форматирует раздел как Ext4, записывая минимальный суперблок.
///
/// Для `/`: записываем суперблок со стандартной сигнатурой 0xEF53.
/// Это достаточно чтобы `Ext4FileSystem::open()` корректно его принял.
/// Полный mkfs.ext4 (группы блоков, inode-таблицы, журнал) — следующий этап разработки.
pub fn format_ext4(
    dev: Arc<Mutex<dyn BlockDevice>>,
    part_start_lba: u64,
    _sector_count: u64,
) -> Result<(), VfsError> {
    // Суперблок Ext4: минимальная структура для распознавания ФС
    // Смещения внутри суперблока (от байта 1024 от начала раздела):
    //   0-3:   s_inodes_count
    //   4-7:   s_blocks_count_lo
    //  24-27:  s_log_block_size (0 = 1 KiB)
    //  56-57:  s_magic = 0xEF53
    //  58-59:  s_state (1 = clean)
    let mut sb = [0u8; 1024];
    sb[0..4].copy_from_slice(&1024u32.to_le_bytes());   // s_inodes_count
    sb[4..8].copy_from_slice(&4096u32.to_le_bytes());   // s_blocks_count_lo
    sb[24..28].copy_from_slice(&EXT4_BLOCK_SIZE_1K.to_le_bytes()); // s_log_block_size
    sb[56..58].copy_from_slice(&EXT4_MAGIC.to_le_bytes());          // s_magic
    sb[58..60].copy_from_slice(&1u16.to_le_bytes());    // s_state = clean

    // Суперблок записывается в LBA 2 от начала раздела (байты 1024-2047 = 2 сектора по 512 байт)
    let target_lba = part_start_lba + 2;
    dev.lock()
        .write_sectors(target_lba, &sb)
        .map_err(|_| VfsError::IoError)?;

    Ok(())
}
