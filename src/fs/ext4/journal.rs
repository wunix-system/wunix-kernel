//! Подсистема журналирования JBD2 (Journaling Block Device v2) для Ext4.
//!
//! Обеспечивает строгую транзакционную целостность метаданных при сбоях питания:
//! модифицируемые блоки метаданных предварительно записываются в циклический буфер
//! журнала с дескриптором и блоком фиксации (commit), после чего сбрасываются на диск.

use alloc::sync::Arc;
use alloc::vec;
use spin::Mutex;
use crate::drivers::BlockDevice;
use super::super::vfs::VfsError;

pub const JBD2_MAGIC_NUMBER: u32 = 0xC036390C;

pub const JBD2_DESCRIPTOR_BLOCK: u32 = 1;
pub const JBD2_COMMIT_BLOCK: u32     = 2;
pub const JBD2_SUPERBLOCK_V2: u32    = 4;

pub const JBD2_FLAG_LAST_TAG: u32 = 0x0008;

#[derive(Debug, Clone)]
pub struct JournalSuperblock {
    pub block_size: u32,
    pub max_len: u32,
    pub first: u32,
    pub sequence: u32,
    pub start: u32,
}

impl JournalSuperblock {
    pub fn parse(buf: &[u8]) -> Result<Self, VfsError> {
        if buf.len() < 1024 {
            return Err(VfsError::IoError);
        }
        let magic = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != JBD2_MAGIC_NUMBER {
            return Err(VfsError::NotFound);
        }
        let block_type = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if block_type != JBD2_SUPERBLOCK_V2 && block_type != 3 {
            return Err(VfsError::IoError);
        }

        Ok(Self {
            block_size: u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]),
            max_len: u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]),
            first: u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]),
            sequence: u32::from_be_bytes([buf[24], buf[25], buf[26], buf[27]]),
            start: u32::from_be_bytes([buf[28], buf[29], buf[30], buf[31]]),
        })
    }

    pub fn write_to_slice(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&JBD2_MAGIC_NUMBER.to_be_bytes());
        buf[4..8].copy_from_slice(&JBD2_SUPERBLOCK_V2.to_be_bytes());
        buf[8..12].copy_from_slice(&self.sequence.to_be_bytes());
        buf[12..16].copy_from_slice(&self.block_size.to_be_bytes());
        buf[16..20].copy_from_slice(&self.max_len.to_be_bytes());
        buf[20..24].copy_from_slice(&self.first.to_be_bytes());
        buf[24..28].copy_from_slice(&self.sequence.to_be_bytes());
        buf[28..32].copy_from_slice(&self.start.to_be_bytes());
    }
}

pub struct Ext4Journal {
    dev: Arc<Mutex<dyn BlockDevice>>,
    journal_start_block: u64,
    pub sb: JournalSuperblock,
}

impl Ext4Journal {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>, journal_start_block: u64) -> Result<Self, VfsError> {
        let mut buf = vec![0u8; 4096];
        let sec = journal_start_block * 8;
        dev.lock().read_sectors(sec, &mut buf).map_err(|_| VfsError::IoError)?;

        let sb = JournalSuperblock::parse(&buf)?;
        let mut journal = Self {
            dev,
            journal_start_block,
            sb,
        };

        // Если журнал был «грязным» (система была некорректно выключена), восстанавливаем транзакции
        if journal.sb.start != 0 {
            journal.recover()?;
        }

        Ok(journal)
    }

    /// Восстановление транзакций из журнала на дисковые блоки ФС (Journal Recovery)
    pub fn recover(&mut self) -> Result<(), VfsError> {
        let mut cur_block = self.sb.start;
        let mut desc_buf = vec![0u8; 4096];
        let mut data_buf = vec![0u8; 4096];

        while cur_block < self.sb.max_len && cur_block != 0 {
            let desc_sec = (self.journal_start_block + cur_block as u64) * 8;
            if self.dev.lock().read_sectors(desc_sec, &mut desc_buf).is_err() {
                break;
            }

            let magic = u32::from_be_bytes([desc_buf[0], desc_buf[1], desc_buf[2], desc_buf[3]]);
            let btype = u32::from_be_bytes([desc_buf[4], desc_buf[5], desc_buf[6], desc_buf[7]]);

            if magic != JBD2_MAGIC_NUMBER || btype != JBD2_DESCRIPTOR_BLOCK {
                break;
            }

            let mut tag_pos = 12;
            let mut log_data_block = cur_block + 1;

            while tag_pos + 8 <= 4096 {
                let target_block = u32::from_be_bytes([
                    desc_buf[tag_pos], desc_buf[tag_pos + 1], desc_buf[tag_pos + 2], desc_buf[tag_pos + 3],
                ]);
                let flags = u32::from_be_bytes([
                    desc_buf[tag_pos + 4], desc_buf[tag_pos + 5], desc_buf[tag_pos + 6], desc_buf[tag_pos + 7],
                ]);

                if target_block != 0 && log_data_block < self.sb.max_len {
                    // Читаем блок из журнала и записываем в целевой блок ФС
                    let log_sec = (self.journal_start_block + log_data_block as u64) * 8;
                    let target_sec = (target_block as u64) * 8;

                    if self.dev.lock().read_sectors(log_sec, &mut data_buf).is_ok() {
                        let _ = self.dev.lock().write_sectors(target_sec, &data_buf);
                    }
                    log_data_block += 1;
                }

                if (flags & JBD2_FLAG_LAST_TAG) != 0 {
                    break;
                }
                tag_pos += 8;
            }

            // Следующий блок после данных — блок фиксации (COMMIT)
            cur_block = log_data_block + 1;
        }

        // Очищаем статус журнала (start = 0 означает чистый сброшенный журнал)
        self.sb.start = 0;
        self.flush_superblock()?;
        Ok(())
    }

    /// Запись атомарной транзакции метаданных через журнал
    pub fn commit_transaction(&mut self, modified_blocks: &[(u64, &[u8])]) -> Result<(), VfsError> {
        if modified_blocks.is_empty() {
            return Ok(());
        }

        let mut desc_buf = vec![0u8; 4096];
        desc_buf[0..4].copy_from_slice(&JBD2_MAGIC_NUMBER.to_be_bytes());
        desc_buf[4..8].copy_from_slice(&JBD2_DESCRIPTOR_BLOCK.to_be_bytes());
        desc_buf[8..12].copy_from_slice(&self.sb.sequence.to_be_bytes());

        let mut tag_pos = 12;
        let mut log_offset = self.sb.first;

        for (idx, (target_block, _)) in modified_blocks.iter().enumerate() {
            let is_last = idx == modified_blocks.len() - 1;
            let flags = if is_last { JBD2_FLAG_LAST_TAG } else { 0 };

            desc_buf[tag_pos..tag_pos + 4].copy_from_slice(&(*target_block as u32).to_be_bytes());
            desc_buf[tag_pos + 4..tag_pos + 8].copy_from_slice(&flags.to_be_bytes());
            tag_pos += 8;
        }

        // 1. Пишем блок дескриптора в журнал
        let desc_sec = (self.journal_start_block + log_offset as u64) * 8;
        self.dev.lock().write_sectors(desc_sec, &desc_buf).map_err(|_| VfsError::IoError)?;
        log_offset += 1;

        // 2. Пишем блоки данных в журнал
        for (_, data) in modified_blocks {
            let data_sec = (self.journal_start_block + log_offset as u64) * 8;
            self.dev.lock().write_sectors(data_sec, data).map_err(|_| VfsError::IoError)?;
            log_offset += 1;
        }

        // 3. Пишем блок фиксации (COMMIT BLOCK)
        let mut commit_buf = vec![0u8; 4096];
        commit_buf[0..4].copy_from_slice(&JBD2_MAGIC_NUMBER.to_be_bytes());
        commit_buf[4..8].copy_from_slice(&JBD2_COMMIT_BLOCK.to_be_bytes());
        commit_buf[8..12].copy_from_slice(&self.sb.sequence.to_be_bytes());

        let commit_sec = (self.journal_start_block + log_offset as u64) * 8;
        self.dev.lock().write_sectors(commit_sec, &commit_buf).map_err(|_| VfsError::IoError)?;

        // 4. Физически сбрасываем модифицированные блоки на их постоянные места на диске
        for (target_block, data) in modified_blocks {
            let target_sec = (*target_block) * 8;
            self.dev.lock().write_sectors(target_sec, data).map_err(|_| VfsError::IoError)?;
        }

        // 5. Инкрементируем sequence
        self.sb.sequence = self.sb.sequence.wrapping_add(1);
        self.flush_superblock()?;
        Ok(())
    }

    fn flush_superblock(&self) -> Result<(), VfsError> {
        let mut buf = vec![0u8; 4096];
        self.sb.write_to_slice(&mut buf);
        let sec = self.journal_start_block * 8;
        self.dev.lock().write_sectors(sec, &buf).map_err(|_| VfsError::IoError)?;
        Ok(())
    }
}
