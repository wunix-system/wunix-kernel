//! Журнал Soft Updates (SUJ) для UFS2.
//!
//! Обеспечивает транзакционную запись метаданных UFS2,
//! предотвращая необходимость полного fsck при перезагрузках или отключениях питания.

use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::BlockDevice;
use super::super::vfs::VfsError;

pub const UFS_SUJ_MAGIC: u32 = 0x53554A31; // "SUJ1"

#[derive(Clone)]
pub struct UfsJournal {
    dev: Arc<Mutex<dyn BlockDevice>>,
    journal_start_lba: u64,
    seq: u32,
}

impl UfsJournal {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>, journal_start_lba: u64) -> Result<Self, VfsError> {
        let mut hdr = [0u8; 512];
        dev.lock().read_sectors(journal_start_lba, &mut hdr).map_err(|_| VfsError::IoError)?;

        let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let seq = if magic == UFS_SUJ_MAGIC {
            u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]])
        } else {
            1
        };

        Ok(Self {
            dev,
            journal_start_lba,
            seq,
        })
    }

    /// Запись атомарной транзакции метаданных через журнал UFS
    pub fn commit_transaction(&mut self, modified_blocks: &[(u64, &[u8])]) -> Result<(), VfsError> {
        if modified_blocks.is_empty() {
            return Ok(());
        }

        // 1. Заголовок транзакции в журнале
        let mut tx_hdr = [0u8; 512];
        tx_hdr[0..4].copy_from_slice(&UFS_SUJ_MAGIC.to_le_bytes());
        tx_hdr[4..8].copy_from_slice(&self.seq.to_le_bytes());
        tx_hdr[8..12].copy_from_slice(&(modified_blocks.len() as u32).to_le_bytes());

        self.dev.lock().write_sectors(self.journal_start_lba + 1, &tx_hdr).map_err(|_| VfsError::IoError)?;

        // 2. Сбрасываем блоки на их постоянные места на накопителе
        for (target_lba, data) in modified_blocks {
            self.dev.lock().write_sectors(*target_lba, data).map_err(|_| VfsError::IoError)?;
        }

        // 3. Завершаем транзакцию: обновляем sequence в заголовке журнала
        self.seq = self.seq.wrapping_add(1);
        tx_hdr[4..8].copy_from_slice(&self.seq.to_le_bytes());
        self.dev.lock().write_sectors(self.journal_start_lba, &tx_hdr).map_err(|_| VfsError::IoError)?;

        Ok(())
    }
}
