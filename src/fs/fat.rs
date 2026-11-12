use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use spin::Mutex;

use crate::drivers::BlockDevice;
use crate::fs::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};

pub const FAT32_ATTR_READ_ONLY: u8 = 0x01;
pub const FAT32_ATTR_HIDDEN: u8 = 0x02;
pub const FAT32_ATTR_SYSTEM: u8 = 0x04;
pub const FAT32_ATTR_VOLUME_ID: u8 = 0x08;
pub const FAT32_ATTR_DIRECTORY: u8 = 0x10;
pub const FAT32_ATTR_ARCHIVE: u8 = 0x20;
pub const FAT32_ATTR_LFN: u8 = 0x0F;

pub const FAT32_EOC: u32 = 0x0FFF_FFFF;

pub struct FatCache {
    pub sector_idx: u64,
    pub data: [u8; 512],
    pub dirty: bool,
}

/// Внутреннее состояние и геометрия смонтированной файловой системы FAT32
pub struct Fat32Inner {
    pub dev: Arc<Mutex<dyn BlockDevice>>,
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub fat_start_lba: u64,
    pub data_start_lba: u64,
    pub cluster_bytes: usize,
    pub last_alloc_cluster: Mutex<u32>,
    pub fat_cache: Mutex<FatCache>,
}

impl Fat32Inner {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        let mut bpb = [0u8; 512];
        dev.lock().read_sectors(0, &mut bpb).map_err(|_| VfsError::IoError)?;

        // Проверяем сигнатуру загрузочного сектора 0x55 0xAA
        if bpb[510] != 0x55 || bpb[511] != 0xAA {
            return Err(VfsError::NotFound);
        }

        let bytes_per_sector = u16::from_le_bytes([bpb[11], bpb[12]]);
        let sectors_per_cluster = bpb[13];
        let reserved_sectors = u16::from_le_bytes([bpb[14], bpb[15]]);
        let num_fats = bpb[16];

        if bytes_per_sector != 512 || sectors_per_cluster == 0 || num_fats == 0 {
            return Err(VfsError::NotFound);
        }

        let sectors_per_fat = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]);
        let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]);

        if sectors_per_fat == 0 || root_cluster < 2 {
            return Err(VfsError::NotFound);
        }

        let fat_start_lba = reserved_sectors as u64;
        let data_start_lba = fat_start_lba + (num_fats as u64 * sectors_per_fat as u64);
        let cluster_bytes = (bytes_per_sector as usize) * (sectors_per_cluster as usize);

        Ok(Self {
            dev,
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            sectors_per_fat,
            root_cluster,
            fat_start_lba,
            data_start_lba,
            cluster_bytes,
            last_alloc_cluster: Mutex::new(2),
            fat_cache: Mutex::new(FatCache {
                sector_idx: u64::MAX,
                data: [0u8; 512],
                dirty: false,
            }),
        })
    }

    /// Преобразует номер кластера данных (2..N) в абсолютный сектор LBA
    pub fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_start_lba + ((cluster as u64 - 2) * self.sectors_per_cluster as u64)
    }

    /// Сбрасывает «грязный» сектор кэша FAT на диск в обе копии таблицы FAT
    pub fn flush_fat_cache(&self) -> Result<(), VfsError> {
        let mut cache = self.fat_cache.lock();
        if cache.dirty && cache.sector_idx != u64::MAX {
            let mut dev = self.dev.lock();
            for f in 0..self.num_fats {
                let fat_sec = self.fat_start_lba + (f as u64 * self.sectors_per_fat as u64) + cache.sector_idx;
                dev.write_sectors(fat_sec, &cache.data).map_err(|_| VfsError::IoError)?;
            }
            cache.dirty = false;
        }
        Ok(())
    }

    fn load_fat_cache_sector(&self, cache: &mut FatCache, sec_idx: u64) -> Result<(), VfsError> {
        if cache.sector_idx == sec_idx {
            return Ok(());
        }
        if cache.dirty && cache.sector_idx != u64::MAX {
            let mut dev = self.dev.lock();
            for f in 0..self.num_fats {
                let fat_sec = self.fat_start_lba + (f as u64 * self.sectors_per_fat as u64) + cache.sector_idx;
                dev.write_sectors(fat_sec, &cache.data).map_err(|_| VfsError::IoError)?;
            }
            cache.dirty = false;
        }
        let fat_sec = self.fat_start_lba + sec_idx;
        self.dev.lock().read_sectors(fat_sec, &mut cache.data).map_err(|_| VfsError::IoError)?;
        cache.sector_idx = sec_idx;
        Ok(())
    }

    /// Считывает значение элемента таблицы FAT для заданного кластера через кэш секторов FAT
    pub fn get_fat_entry(&self, cluster: u32) -> Result<u32, VfsError> {
        let fat_offset = (cluster as u64) * 4;
        let sec_idx = fat_offset / 512;
        let offset_in_sec = (fat_offset % 512) as usize;

        let mut cache = self.fat_cache.lock();
        self.load_fat_cache_sector(&mut cache, sec_idx)?;

        let val = u32::from_le_bytes([
            cache.data[offset_in_sec],
            cache.data[offset_in_sec + 1],
            cache.data[offset_in_sec + 2],
            cache.data[offset_in_sec + 3],
        ]);

        Ok(val & 0x0FFF_FFFF)
    }

    /// Записывает новое значение в таблицу FAT через кэш секторов
    pub fn set_fat_entry(&self, cluster: u32, value: u32) -> Result<(), VfsError> {
        let fat_offset = (cluster as u64) * 4;
        let sec_idx = fat_offset / 512;
        let offset_in_sec = (fat_offset % 512) as usize;
        let val_bytes = (value & 0x0FFF_FFFF).to_le_bytes();

        let mut cache = self.fat_cache.lock();
        self.load_fat_cache_sector(&mut cache, sec_idx)?;
        cache.data[offset_in_sec..offset_in_sec + 4].copy_from_slice(&val_bytes);
        cache.dirty = true;

        Ok(())
    }

    /// Сканирует таблицу FAT и выделяет первый свободный кластер (значение 0).
    /// Оптимизировано: чтение таблицы FAT целыми секторами через кэш FAT
    /// и кэширование курсора для мгновенного линейного выделения кластеров без O(N^2) I/O.
    pub fn alloc_cluster(&self) -> Result<u32, VfsError> {
        let max_clusters = (self.sectors_per_fat as u64 * 512) / 4;
        let total_fat_secs = self.sectors_per_fat as u64;

        let mut cursor = self.last_alloc_cluster.lock();
        let start_cluster = if *cursor >= 2 && (*cursor as u64) < max_clusters {
            *cursor
        } else {
            2
        };

        let start_sec = (start_cluster as u64 * 4) / 512;
        let mut cache = self.fat_cache.lock();

        for s in 0..total_fat_secs {
            let sec_idx = (start_sec + s) % total_fat_secs;
            self.load_fat_cache_sector(&mut cache, sec_idx)?;

            for entry_idx in 0..128 {
                let cluster_num = (sec_idx * 128 + entry_idx as u64) as u32;
                if cluster_num < 2 || (cluster_num as u64) >= max_clusters {
                    continue;
                }
                let off = entry_idx * 4;
                let val = u32::from_le_bytes([
                    cache.data[off],
                    cache.data[off + 1],
                    cache.data[off + 2],
                    cache.data[off + 3],
                ]) & 0x0FFF_FFFF;

                if val == 0 {
                    *cursor = cluster_num + 1;
                    cache.data[off..off + 4].copy_from_slice(&(FAT32_EOC & 0x0FFF_FFFF).to_le_bytes());
                    cache.dirty = true;
                    return Ok(cluster_num);
                }
            }
        }
        Err(VfsError::IoError) // Диск переполнен
    }

    /// Освобождает цепочку кластеров, начиная с `start_cluster`
    pub fn free_cluster_chain(&self, start_cluster: u32) -> Result<(), VfsError> {
        let mut curr = start_cluster;
        while curr >= 2 && curr < 0x0FFF_FFF8 {
            let next = self.get_fat_entry(curr)?;
            self.set_fat_entry(curr, 0)?;
            curr = next;
        }
        self.flush_fat_cache()?;
        Ok(())
    }

    /// Считывает полный кластер данных
    pub fn read_cluster(&self, cluster: u32, buf: &mut [u8]) -> Result<(), VfsError> {
        if buf.len() < self.cluster_bytes {
            return Err(VfsError::IoError);
        }
        let lba = self.cluster_to_lba(cluster);
        self.dev
            .lock()
            .read_sectors(lba, &mut buf[..self.cluster_bytes])
            .map_err(|_| VfsError::IoError)?;
        Ok(())
    }

    /// Записывает полный кластер данных
    pub fn write_cluster(&self, cluster: u32, buf: &[u8]) -> Result<(), VfsError> {
        if buf.len() < self.cluster_bytes {
            return Err(VfsError::IoError);
        }
        let lba = self.cluster_to_lba(cluster);
        self.dev
            .lock()
            .write_sectors(lba, &buf[..self.cluster_bytes])
            .map_err(|_| VfsError::IoError)?;
        Ok(())
    }

    /// Возвращает вектор всех номеров кластеров в цепочке
    pub fn get_cluster_chain(&self, start: u32) -> Result<Vec<u32>, VfsError> {
        let mut chain = Vec::new();
        if start < 2 {
            return Ok(chain);
        }
        let mut curr = start;
        let mut depth = 0;
        while curr >= 2 && curr < 0x0FFF_FFF8 {
            chain.push(curr);
            curr = self.get_fat_entry(curr)?;
            depth += 1;
            if depth > 500_000 {
                return Err(VfsError::IoError);
            }
        }
        Ok(chain)
    }
}

/// 32-битная запись каталога FAT (Directory Entry)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawDirEntry {
    pub name: [u8; 8],
    pub ext: [u8; 3],
    pub attr: u8,
    pub reserved: u8,
    pub ctime_tenth: u8,
    pub ctime: u16,
    pub cdate: u16,
    pub adate: u16,
    pub cluster_high: u16,
    pub mtime: u16,
    pub mdate: u16,
    pub cluster_low: u16,
    pub file_size: u32,
}

/// Запись длинного имени файла VFAT LFN (32 байта)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LfnEntry {
    pub seq: u8,
    pub name1: [u16; 5],
    pub attr: u8,           // Всегда 0x0F (FAT32_ATTR_LFN)
    pub lfn_type: u8,       // Всегда 0x00
    pub checksum: u8,       // Контрольная сумма 8.3 имени
    pub name2: [u16; 6],
    pub first_cluster: u16, // Всегда 0x0000
    pub name3: [u16; 2],
}

/// Вычисляет контрольную сумму короткого имени 8.3 для связки с LFN
pub fn lfn_checksum(name: &[u8; 8], ext: &[u8; 3]) -> u8 {
    let mut sum: u8 = 0;
    for &b in name.iter().chain(ext.iter()) {
        sum = (sum >> 1).wrapping_add(sum << 7).wrapping_add(b);
    }
    sum
}

#[derive(Clone, Debug)]
pub struct ParsedDirEntry {
    pub offset: usize,
    pub raw: RawDirEntry,
    pub lfn: Option<String>,
}

impl ParsedDirEntry {
    pub fn display_name(&self) -> String {
        if let Some(ref lfn) = self.lfn {
            lfn.clone()
        } else {
            self.raw.to_display_name()
        }
    }
}

impl RawDirEntry {
    pub fn first_cluster(&self) -> u32 {
        ((self.cluster_high as u32) << 16) | (self.cluster_low as u32)
    }

    pub fn set_first_cluster(&mut self, clus: u32) {
        self.cluster_high = ((clus >> 16) & 0xFFFF) as u16;
        self.cluster_low = (clus & 0xFFFF) as u16;
    }

    /// Преобразует 8.3 представление имени в стандартную строку
    pub fn to_display_name(&self) -> String {
        let name_trimmed = core::str::from_utf8(&self.name)
            .unwrap_or("")
            .trim_end_matches(' ');
        let ext_trimmed = core::str::from_utf8(&self.ext)
            .unwrap_or("")
            .trim_end_matches(' ');

        let mut res = String::new();
        for c in name_trimmed.chars() {
            res.push(c.to_ascii_lowercase());
        }
        if !ext_trimmed.is_empty() {
            res.push('.');
            for c in ext_trimmed.chars() {
                res.push(c.to_ascii_lowercase());
            }
        }
        res
    }
}

/// Форматирует обычное имя файла (например, "test.txt") в 8.3 DOS-совместимый вид
pub fn to_dos_83(name: &str) -> Option<([u8; 8], [u8; 3])> {
    let mut name_buf = [b' '; 8];
    let mut ext_buf = [b' '; 3];

    let parts: Vec<&str> = name.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let base = parts[0];
    let mut base_idx = 0;
    for b in base.bytes() {
        if base_idx >= 8 {
            break;
        }
        name_buf[base_idx] = b.to_ascii_uppercase();
        base_idx += 1;
    }

    if parts.len() > 1 {
        let ext = parts[1];
        let mut ext_idx = 0;
        for b in ext.bytes() {
            if ext_idx >= 3 {
                break;
            }
            ext_buf[ext_idx] = b.to_ascii_uppercase();
            ext_idx += 1;
        }
    }

    Some((name_buf, ext_buf))
}

/// Генерирует уникальное 8.3 короткое имя для файла в каталоге FAT32,
/// предотвращая коллизии псевдонимов коротких имен при использовании LFN (длинных имен).
pub fn generate_unique_dos_83(name: &str, existing: &[ParsedDirEntry]) -> Option<([u8; 8], [u8; 3])> {
    let mut ext_buf = [b' '; 3];
    let (base_part, ext_part) = match name.rfind('.') {
        Some(idx) => (&name[..idx], &name[idx + 1..]),
        None => (name, ""),
    };

    let mut ext_idx = 0;
    for b in ext_part.bytes() {
        if ext_idx >= 3 {
            break;
        }
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            ext_buf[ext_idx] = b.to_ascii_uppercase();
            ext_idx += 1;
        }
    }

    let mut clean_base = alloc::vec::Vec::new();
    for b in base_part.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            clean_base.push(b.to_ascii_uppercase());
        }
    }
    if clean_base.is_empty() {
        clean_base.push(b'_');
    }

    // Если исходное имя изначально укладывается в 8.3 без спецсимволов и не занято
    let is_simple = name.len() <= 12
        && base_part.len() <= 8
        && ext_part.len() <= 3
        && base_part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');

    if is_simple {
        let mut candidate = [b' '; 8];
        for (i, &b) in clean_base.iter().take(8).enumerate() {
            candidate[i] = b;
        }
        let exists = existing.iter().any(|e| e.raw.name == candidate && e.raw.ext == ext_buf);
        if !exists {
            return Some((candidate, ext_buf));
        }
    }

    // При длинных именах или коллизиях генерируем числовой хвост ~1, ~2 ... ~999
    for i in 1..=999usize {
        let mut tail = [0u8; 5];
        let tail_len = {
            use core::fmt::Write;
            struct BufWriter<'a> {
                buf: &'a mut [u8],
                pos: usize,
            }
            impl<'a> core::fmt::Write for BufWriter<'a> {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    for b in s.bytes() {
                        if self.pos < self.buf.len() {
                            self.buf[self.pos] = b;
                            self.pos += 1;
                        }
                    }
                    Ok(())
                }
            }
            let mut w = BufWriter { buf: &mut tail, pos: 0 };
            let _ = write!(w, "~{}", i);
            w.pos
        };

        let prefix_len = 8usize.saturating_sub(tail_len);
        let mut candidate = [b' '; 8];
        let copy_len = clean_base.len().min(prefix_len);
        candidate[..copy_len].copy_from_slice(&clean_base[..copy_len]);
        candidate[copy_len..copy_len + tail_len].copy_from_slice(&tail[..tail_len]);

        let exists = existing.iter().any(|e| e.raw.name == candidate && e.raw.ext == ext_buf);
        if !exists {
            return Some((candidate, ext_buf));
        }
    }

    None
}

fn default_dir_permissions(name: &str) -> (u16, u32, u32) {
    if name.eq_ignore_ascii_case("tmp") {
        (0o1777, 0, 0)
    } else if name.eq_ignore_ascii_case("admin") {
        (0o755, 1000, 1000)
    } else if name.eq_ignore_ascii_case("root") {
        (0o700, 0, 0)
    } else {
        (0o755, 0, 0)
    }
}

fn default_file_permissions(name: &str) -> (u16, u32, u32) {
    let is_suid = name.eq_ignore_ascii_case("sudo")
        || name.eq_ignore_ascii_case("su")
        || name.eq_ignore_ascii_case("passwd")
        || name.eq_ignore_ascii_case("sudoadd");
    let perms = if is_suid {
        0o4755
    } else if name.eq_ignore_ascii_case("shadow")
        || (name.starts_with("ssh_host_") && !name.ends_with(".pub"))
        || name.ends_with(".key")
        || name.ends_with("_key")
    {
        0o600
    } else {
        0o755
    };
    (perms, 0, 0)
}

/// Индексный узел обычного файла на FAT32
pub struct Fat32FileInode {
    pub inner: Arc<Fat32Inner>,
    pub start_cluster: Mutex<u32>,
    pub file_size: Mutex<usize>,
    pub parent_cluster: u32,
    pub dir_entry_offset: usize,
    pub inode_id: u64,
    pub cluster_chain: Mutex<Vec<u32>>,
    pub permissions: AtomicU16,
    pub uid: AtomicU32,
    pub gid: AtomicU32,
}

impl Inode for Fat32FileInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: InodeType::File,
            size: *self.file_size.lock(),
            inode_id: self.inode_id,
            permissions: self.permissions.load(Ordering::Relaxed),
            uid: self.uid.load(Ordering::Relaxed),
            gid: self.gid.load(Ordering::Relaxed),
            rdev: 0,
        }
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        self.permissions.store(mode, Ordering::Relaxed);
        Ok(())
    }

    fn chown(&self, uid: u32, gid: u32) -> Result<(), VfsError> {
        self.uid.store(uid, Ordering::Relaxed);
        self.gid.store(gid, Ordering::Relaxed);
        Ok(())
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let size = *self.file_size.lock();
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }

        let to_read = buf.len().min(size - offset);
        let start_clus = *self.start_cluster.lock();
        if start_clus < 2 {
            return Ok(0);
        }

        let mut chain_guard = self.cluster_chain.lock();
        if chain_guard.is_empty() {
            *chain_guard = self.inner.get_cluster_chain(start_clus)?;
        }
        let chain = chain_guard.clone();
        drop(chain_guard);

        let clus_size = self.inner.cluster_bytes;
        let mut bytes_read = 0usize;
        let mut clus_buf = alloc::vec![0u8; clus_size];

        while bytes_read < to_read {
            let curr_offset = offset + bytes_read;
            let clus_idx = curr_offset / clus_size;
            let offset_in_clus = curr_offset % clus_size;
            let chunk = (clus_size - offset_in_clus).min(to_read - bytes_read);

            if clus_idx >= chain.len() {
                break;
            }

            self.inner.read_cluster(chain[clus_idx], &mut clus_buf)?;
            buf[bytes_read..bytes_read + chunk]
                .copy_from_slice(&clus_buf[offset_in_clus..offset_in_clus + chunk]);

            bytes_read += chunk;
        }

        Ok(bytes_read)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut start_clus = *self.start_cluster.lock();
        if start_clus < 2 {
            start_clus = self.inner.alloc_cluster()?;
            *self.start_cluster.lock() = start_clus;
        }

        let clus_size = self.inner.cluster_bytes;
        let mut chain_guard = self.cluster_chain.lock();
        if chain_guard.is_empty() {
            *chain_guard = self.inner.get_cluster_chain(start_clus)?;
        }

        let end_offset = offset + buf.len();
        let needed_clusters = (end_offset + clus_size - 1) / clus_size;

        while chain_guard.len() < needed_clusters {
            let last = *chain_guard.last().unwrap_or(&start_clus);
            let next = self.inner.alloc_cluster()?;
            self.inner.set_fat_entry(last, next)?;
            chain_guard.push(next);
        }

        let chain = chain_guard.clone();
        drop(chain_guard);

        let mut bytes_written = 0usize;
        let mut clus_buf = alloc::vec![0u8; clus_size];

        while bytes_written < buf.len() {
            let curr_offset = offset + bytes_written;
            let clus_idx = curr_offset / clus_size;
            let offset_in_clus = curr_offset % clus_size;
            let chunk = (clus_size - offset_in_clus).min(buf.len() - bytes_written);

            let clus_id = chain[clus_idx];
            if offset_in_clus != 0 || chunk < clus_size {
                self.inner.read_cluster(clus_id, &mut clus_buf)?;
                clus_buf[offset_in_clus..offset_in_clus + chunk]
                    .copy_from_slice(&buf[bytes_written..bytes_written + chunk]);
                self.inner.write_cluster(clus_id, &clus_buf)?;
            } else {
                // Прямая запись полного кластера без чтения и промежуточного буфера
                self.inner.write_cluster(clus_id, &buf[bytes_written..bytes_written + chunk])?;
            }

            bytes_written += chunk;
        }

        let _ = self.inner.flush_fat_cache();

        let mut size_guard = self.file_size.lock();
        if end_offset > *size_guard {
            *size_guard = end_offset;

            // Обновляем размер файла и начальный кластер в записи родительского каталога на диске
            if self.parent_cluster >= 2 {
                let parent_chain = self.inner.get_cluster_chain(self.parent_cluster)?;
                let clus_idx = self.dir_entry_offset / clus_size;
                let offset_in_clus = self.dir_entry_offset % clus_size;

                if clus_idx < parent_chain.len() {
                    let mut dir_buf = alloc::vec![0u8; clus_size];
                    let dir_clus = parent_chain[clus_idx];
                    let _ = self.inner.read_cluster(dir_clus, &mut dir_buf);

                    let entry_ptr = unsafe {
                        &mut *(dir_buf.as_mut_ptr().add(offset_in_clus) as *mut RawDirEntry)
                    };
                    entry_ptr.file_size = end_offset as u32;
                    entry_ptr.set_first_cluster(start_clus);

                    let _ = self.inner.write_cluster(dir_clus, &dir_buf);
                }
            }
        }

        Ok(bytes_written)
    }

    fn truncate(&self, len: usize) -> Result<(), VfsError> {
        let mut size_guard = self.file_size.lock();
        *size_guard = len;
        if len == 0 {
            self.cluster_chain.lock().clear();
        }
        let start_clus = *self.start_cluster.lock();
        let clus_size = self.inner.cluster_bytes;

        if self.parent_cluster >= 2 {
            let parent_chain = self.inner.get_cluster_chain(self.parent_cluster)?;
            let clus_idx = self.dir_entry_offset / clus_size;
            let offset_in_clus = self.dir_entry_offset % clus_size;

            if clus_idx < parent_chain.len() {
                let mut dir_buf = alloc::vec![0u8; clus_size];
                let dir_clus = parent_chain[clus_idx];
                let _ = self.inner.read_cluster(dir_clus, &mut dir_buf);

                let entry_ptr = unsafe {
                    &mut *(dir_buf.as_mut_ptr().add(offset_in_clus) as *mut RawDirEntry)
                };
                entry_ptr.file_size = len as u32;
                entry_ptr.set_first_cluster(start_clus);

                let _ = self.inner.write_cluster(dir_clus, &dir_buf);
            }
        }
        Ok(())
    }
}

/// Индексный узел директории на FAT32
pub struct Fat32DirInode {
    pub inner: Arc<Fat32Inner>,
    pub cluster: u32,
    pub parent_cluster: u32,
    pub inode_id: u64,
    pub permissions: AtomicU16,
    pub uid: AtomicU32,
    pub gid: AtomicU32,
}

impl Fat32DirInode {
    /// Считывает все записи каталога из цепочки кластеров с декодированием длинных имен (LFN)
    fn read_parsed_entries(&self) -> Result<Vec<ParsedDirEntry>, VfsError> {
        let chain = self.inner.get_cluster_chain(self.cluster)?;
        let mut list = Vec::new();
        let clus_size = self.inner.cluster_bytes;
        let mut clus_buf = alloc::vec![0u8; clus_size];

        let mut lfn_chars = alloc::vec![0u16; 260];
        let mut has_lfn = false;
        let mut expected_checksum = 0u8;

        let mut global_offset = 0usize;
        for c in chain {
            self.inner.read_cluster(c, &mut clus_buf)?;
            let entries_count = clus_size / 32;

            for i in 0..entries_count {
                let offset = i * 32;
                let first_byte = clus_buf[offset];

                if first_byte == 0x00 {
                    return Ok(list);
                }
                if first_byte == 0xE5 {
                    has_lfn = false;
                    global_offset += 32;
                    continue;
                }

                let attr = clus_buf[offset + 11];
                if attr == FAT32_ATTR_LFN {
                    let lfn = unsafe { *(clus_buf.as_ptr().add(offset) as *const LfnEntry) };
                    let seq_num = (lfn.seq & 0x1F) as usize;
                    if (1..=20).contains(&seq_num) {
                        let chunk_idx = seq_num - 1;
                        let base_char_idx = chunk_idx * 13;
                        let name1 = lfn.name1;
                        let name2 = lfn.name2;
                        let name3 = lfn.name3;
                        lfn_chars[base_char_idx..base_char_idx + 5].copy_from_slice(&name1);
                        lfn_chars[base_char_idx + 5..base_char_idx + 11].copy_from_slice(&name2);
                        lfn_chars[base_char_idx + 11..base_char_idx + 13].copy_from_slice(&name3);
                        has_lfn = true;
                        expected_checksum = lfn.checksum;
                    }
                    global_offset += 32;
                    continue;
                }

                if (attr & FAT32_ATTR_VOLUME_ID) != 0 {
                    has_lfn = false;
                    global_offset += 32;
                    continue;
                }

                let raw = unsafe { *(clus_buf.as_ptr().add(offset) as *const RawDirEntry) };
                let mut lfn_name: Option<String> = None;
                if has_lfn && lfn_checksum(&raw.name, &raw.ext) == expected_checksum {
                    let mut s = String::new();
                    for &ch in &lfn_chars {
                        if ch == 0 || ch == 0xFFFF {
                            break;
                        }
                        if let Some(c) = char::from_u32(ch as u32) {
                            s.push(c);
                        }
                    }
                    if !s.is_empty() {
                        lfn_name = Some(s);
                    }
                }

                has_lfn = false;
                lfn_chars.fill(0);

                list.push(ParsedDirEntry {
                    offset: global_offset,
                    raw,
                    lfn: lfn_name,
                });
                global_offset += 32;
            }
        }

        Ok(list)
    }
}

impl Inode for Fat32DirInode {
    fn metadata(&self) -> Metadata {
        Metadata {
            itype: InodeType::Directory,
            size: 4096,
            inode_id: self.inode_id,
            permissions: self.permissions.load(Ordering::Relaxed),
            uid: self.uid.load(Ordering::Relaxed),
            gid: self.gid.load(Ordering::Relaxed),
            rdev: 0,
        }
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        self.permissions.store(mode, Ordering::Relaxed);
        Ok(())
    }

    fn chown(&self, uid: u32, gid: u32) -> Result<(), VfsError> {
        self.uid.store(uid, Ordering::Relaxed);
        self.gid.store(gid, Ordering::Relaxed);
        Ok(())
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        if name == "." {
            return Ok(Arc::new(Fat32DirInode {
                inner: self.inner.clone(),
                cluster: self.cluster,
                parent_cluster: self.parent_cluster,
                inode_id: self.inode_id,
                permissions: AtomicU16::new(self.permissions.load(Ordering::Relaxed)),
                uid: AtomicU32::new(self.uid.load(Ordering::Relaxed)),
                gid: AtomicU32::new(self.gid.load(Ordering::Relaxed)),
            }));
        }

        let entries = self.read_parsed_entries()?;
        for entry in entries {
            let display = entry.display_name();
            let matches = display.eq_ignore_ascii_case(name)
                || entry.raw.to_display_name().eq_ignore_ascii_case(name);
            if matches {
                let first_clus = entry.raw.first_cluster();
                let is_dir = (entry.raw.attr & FAT32_ATTR_DIRECTORY) != 0;

                if is_dir {
                    let (perms, uid, gid) = default_dir_permissions(&display);
                    return Ok(Arc::new(Fat32DirInode {
                        inner: self.inner.clone(),
                        cluster: first_clus,
                        parent_cluster: self.cluster,
                        inode_id: (first_clus as u64) + 1000,
                        permissions: AtomicU16::new(perms),
                        uid: AtomicU32::new(uid),
                        gid: AtomicU32::new(gid),
                    }));
                } else {
                    let (perms, uid, gid) = default_file_permissions(&display);
                    return Ok(Arc::new(Fat32FileInode {
                        inner: self.inner.clone(),
                        start_cluster: Mutex::new(first_clus),
                        file_size: Mutex::new(entry.raw.file_size as usize),
                        parent_cluster: self.cluster,
                        dir_entry_offset: entry.offset,
                        inode_id: (first_clus as u64) + 2000,
                        cluster_chain: Mutex::new(alloc::vec::Vec::new()),
                        permissions: AtomicU16::new(perms),
                        uid: AtomicU32::new(uid),
                        gid: AtomicU32::new(gid),
                    }));
                }
            }
        }

        Err(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let entries = self.read_parsed_entries()?;
        let mut res = Vec::new();

        for entry in entries {
            let name = entry.display_name();
            let is_dir = (entry.raw.attr & FAT32_ATTR_DIRECTORY) != 0;
            let itype = if is_dir {
                InodeType::Directory
            } else {
                InodeType::File
            };
            res.push(DirEntry {
                name,
                inode_id: (entry.raw.first_cluster() as u64) + 1000,
                itype,
            });
        }

        Ok(res)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        // Проверяем наличие файла/директории с таким именем: предотвращаем появление дубликатов в каталоге
        if self.lookup(name).is_ok() {
            return Err(VfsError::AlreadyExists);
        }

        let name_utf16: Vec<u16> = name.encode_utf16().collect();
        let num_lfn = (name_utf16.len() + 12) / 13;
        let slots_needed = 1 + num_lfn;

        let mut chain = self.inner.get_cluster_chain(self.cluster)?;
        let clus_size = self.inner.cluster_bytes;
        let mut clus_buf = alloc::vec![0u8; clus_size];

        let mut target_clus = 0u32;
        let mut target_clus_idx = 0usize;
        let mut target_slot_idx = 0usize;
        let mut found = false;

        'search: for (clus_idx, c) in chain.iter().enumerate() {
            self.inner.read_cluster(*c, &mut clus_buf)?;
            let total_entries = clus_size / 32;
            let mut consecutive = 0;
            let mut start_idx = 0;

            for i in 0..total_entries {
                let b = clus_buf[i * 32];
                if b == 0x00 || b == 0xE5 {
                    if consecutive == 0 {
                        start_idx = i;
                    }
                    consecutive += 1;
                    if consecutive == slots_needed {
                        target_clus = *c;
                        target_clus_idx = clus_idx;
                        target_slot_idx = start_idx;
                        found = true;
                        break 'search;
                    }
                } else {
                    consecutive = 0;
                }
            }
        }

        if !found {
            // Выделяем новый кластер для каталога и сразу обнуляем его, исключая мусорные записи
            let new_c = self.inner.alloc_cluster()?;
            let last = *chain.last().unwrap_or(&self.cluster);
            self.inner.set_fat_entry(last, new_c)?;
            self.inner.flush_fat_cache()?;
            target_clus_idx = chain.len();
            chain.push(new_c);
            target_clus = new_c;
            target_slot_idx = 0;
            clus_buf.fill(0);
            self.inner.write_cluster(target_clus, &clus_buf)?;
        }

        // Генерируем уникальное 8.3 короткое имя файла с суффиксом ~1..~999 при необходимости
        let existing_entries = self.read_parsed_entries()?;
        let (dos_name, dos_ext) = match generate_unique_dos_83(name, &existing_entries) {
            Some(p) => p,
            None => return Err(VfsError::InvalidPath),
        };
        let checksum = lfn_checksum(&dos_name, &dos_ext);

        let mut cur_slot = target_slot_idx;

        // 1. Записываем LFN записи в обратном порядке (от N до 1)
        if num_lfn > 0 {
            for entry_idx in (1..=num_lfn).rev() {
                let is_last = entry_idx == num_lfn;
                let seq = (entry_idx as u8) | (if is_last { 0x40 } else { 0x00 });
                let char_start = (entry_idx - 1) * 13;

                let mut name1 = [0xFFFFu16; 5];
                let mut name2 = [0xFFFFu16; 6];
                let mut name3 = [0xFFFFu16; 2];

                for j in 0..13 {
                    let ch_idx = char_start + j;
                    let val = if ch_idx < name_utf16.len() {
                        name_utf16[ch_idx]
                    } else if ch_idx == name_utf16.len() {
                        0x0000
                    } else {
                        0xFFFF
                    };
                    if j < 5 {
                        name1[j] = val;
                    } else if j < 11 {
                        name2[j - 5] = val;
                    } else {
                        name3[j - 11] = val;
                    }
                }

                let lfn = LfnEntry {
                    seq,
                    name1,
                    attr: FAT32_ATTR_LFN,
                    lfn_type: 0,
                    checksum,
                    name2,
                    first_cluster: 0,
                    name3,
                };

                unsafe {
                    let ptr = clus_buf.as_mut_ptr().add(cur_slot * 32) as *mut LfnEntry;
                    core::ptr::write(ptr, lfn);
                }
                cur_slot += 1;
            }
        }

        // 2. Записываем стандартную запись каталога RawDirEntry
        let is_dir = itype == InodeType::Directory;
        let first_clus = if is_dir {
            let clus = self.inner.alloc_cluster()?;
            let mut new_dir_buf = alloc::vec![0u8; self.inner.cluster_bytes];
            let dot_raw = RawDirEntry {
                name: *b".       ",
                ext: *b"   ",
                attr: FAT32_ATTR_DIRECTORY,
                reserved: 0,
                ctime_tenth: 0,
                ctime: 0,
                cdate: 0x5500,
                adate: 0x5500,
                cluster_high: ((clus >> 16) & 0xFFFF) as u16,
                mtime: 0,
                mdate: 0x5500,
                cluster_low: (clus & 0xFFFF) as u16,
                file_size: 0,
            };
            let dotdot_raw = RawDirEntry {
                name: *b"..      ",
                ext: *b"   ",
                attr: FAT32_ATTR_DIRECTORY,
                reserved: 0,
                ctime_tenth: 0,
                ctime: 0,
                cdate: 0x5500,
                adate: 0x5500,
                cluster_high: ((self.cluster >> 16) & 0xFFFF) as u16,
                mtime: 0,
                mdate: 0x5500,
                cluster_low: (self.cluster & 0xFFFF) as u16,
                file_size: 0,
            };
            unsafe {
                let ptr = new_dir_buf.as_mut_ptr() as *mut RawDirEntry;
                core::ptr::write(ptr, dot_raw);
                core::ptr::write(ptr.add(1), dotdot_raw);
            }
            self.inner.write_cluster(clus, &new_dir_buf)?;
            clus
        } else {
            0
        };

        let raw = RawDirEntry {
            name: dos_name,
            ext: dos_ext,
            attr: if is_dir {
                FAT32_ATTR_DIRECTORY
            } else {
                FAT32_ATTR_ARCHIVE
            },
            reserved: 0,
            ctime_tenth: 0,
            ctime: 0,
            cdate: 0x5500,
            adate: 0x5500,
            cluster_high: ((first_clus >> 16) & 0xFFFF) as u16,
            mtime: 0,
            mdate: 0x5500,
            cluster_low: (first_clus & 0xFFFF) as u16,
            file_size: 0,
        };

        let raw_dir_offset = target_clus_idx * clus_size + cur_slot * 32;

        unsafe {
            let ptr = clus_buf.as_mut_ptr().add(cur_slot * 32) as *mut RawDirEntry;
            core::ptr::write(ptr, raw);
        }

        self.inner.write_cluster(target_clus, &clus_buf)?;

        let euid = crate::task::current_thread_euid();
        let egid = crate::task::current_thread_egid();

        if is_dir {
            let (default_perms, default_uid, default_gid) = default_dir_permissions(name);
            let uid = if euid != 0 { euid } else { default_uid };
            let gid = if egid != 0 { egid } else { default_gid };
            Ok(Arc::new(Fat32DirInode {
                inner: self.inner.clone(),
                cluster: first_clus,
                parent_cluster: self.cluster,
                inode_id: (first_clus as u64) + 1000,
                permissions: AtomicU16::new(default_perms),
                uid: AtomicU32::new(uid),
                gid: AtomicU32::new(gid),
            }))
        } else {
            let (default_perms, _, _) = default_file_permissions(name);
            Ok(Arc::new(Fat32FileInode {
                inner: self.inner.clone(),
                start_cluster: Mutex::new(first_clus),
                file_size: Mutex::new(0),
                parent_cluster: self.cluster,
                dir_entry_offset: raw_dir_offset,
                inode_id: (first_clus as u64) + 2000,
                cluster_chain: Mutex::new(alloc::vec::Vec::new()),
                permissions: AtomicU16::new(default_perms),
                uid: AtomicU32::new(euid),
                gid: AtomicU32::new(egid),
            }))
        }
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let entries = self.read_parsed_entries()?;
        let target = entries.into_iter().find(|e| {
            e.display_name().eq_ignore_ascii_case(name)
                || e.raw.to_display_name().eq_ignore_ascii_case(name)
        });

        let target_entry = match target {
            Some(e) => e,
            None => return Err(VfsError::NotFound),
        };

        let chain = self.inner.get_cluster_chain(self.cluster)?;
        let clus_size = self.inner.cluster_bytes;
        let mut clus_buf = alloc::vec![0u8; clus_size];

        let num_lfn = if let Some(ref s) = target_entry.lfn {
            (s.encode_utf16().count() + 12) / 13
        } else {
            0
        };
        let slots_to_delete = 1 + num_lfn;
        let start_offset = target_entry.offset.saturating_sub(num_lfn * 32);

        for slot_idx in 0..slots_to_delete {
            let cur_offset = start_offset + slot_idx * 32;
            let c_idx = cur_offset / clus_size;
            let off_in_c = cur_offset % clus_size;

            if c_idx < chain.len() {
                let clus = chain[c_idx];
                self.inner.read_cluster(clus, &mut clus_buf)?;
                clus_buf[off_in_c] = 0xE5;
                self.inner.write_cluster(clus, &clus_buf)?;
            }
        }

        let first_clus = target_entry.raw.first_cluster();
        if first_clus >= 2 {
            let _ = self.inner.free_cluster_chain(first_clus);
        }

        Ok(())
    }
}

/// Корневая реализация интерфейса `FileSystem` для постоянного накопителя FAT32
pub struct Fat32FileSystem {
    pub inner: Arc<Fat32Inner>,
    root_node: Arc<dyn Inode>,
}

impl Fat32FileSystem {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        let inner = Arc::new(Fat32Inner::open(dev)?);
        let root_node = Arc::new(Fat32DirInode {
            inner: inner.clone(),
            cluster: inner.root_cluster,
            parent_cluster: 0,
            inode_id: 1,
            permissions: AtomicU16::new(0o777),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
        });

        Ok(Self { inner, root_node })
    }
}

impl FileSystem for Fat32FileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.root_node.clone()
    }

    fn name(&self) -> &'static str {
        "fat32"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lfn_checksum() {
        let name = *b"LIMINE  ";
        let ext = *b"SYS";
        let cs = lfn_checksum(&name, &ext);
        assert_eq!(cs, lfn_checksum(&name, &ext));
    }

    #[test]
    fn test_to_dos_83_truncation() {
        let (name, ext) = to_dos_83("limine-bios.sys").unwrap();
        assert_eq!(&name, b"LIMINE-B");
        assert_eq!(&ext, b"SYS");
    }

    #[test]
    fn test_generate_unique_dos_83_collision() {
        let existing = alloc::vec::Vec::new();
        let (n1, e1) = generate_unique_dos_83("ssh_host_ed25519_key", &existing).unwrap();
        assert_eq!(&n1, b"SSH_HO~1");
        assert_eq!(&e1, b"   ");

        let mut existing_with_first = alloc::vec::Vec::new();
        existing_with_first.push(ParsedDirEntry {
            offset: 0,
            raw: RawDirEntry {
                name: n1,
                ext: e1,
                attr: 0,
                reserved: 0,
                ctime_tenth: 0,
                ctime: 0,
                cdate: 0,
                adate: 0,
                cluster_high: 0,
                mtime: 0,
                mdate: 0,
                cluster_low: 3,
                file_size: 100,
            },
            lfn: Some(String::from("ssh_host_ed25519_key")),
        });

        let (n2, e2) = generate_unique_dos_83("ssh_host_rsa_key", &existing_with_first).unwrap();
        assert_eq!(&n2, b"SSH_HO~2");
        assert_eq!(&e2, b"   ");
        assert_ne!(n1, n2);
    }

    #[test]
    fn test_lfn_entry_size() {
        assert_eq!(core::mem::size_of::<LfnEntry>(), 32);
        assert_eq!(core::mem::size_of::<RawDirEntry>(), 32);
    }
}
