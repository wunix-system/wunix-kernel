use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::drivers::{BlockDevice, BlockDeviceError};
use crate::fs::vfs::{Inode, InodeType, Metadata, VfsError};
use crate::serial_println;

/// Блочный накопитель или логический раздел как индексный узел VFS (`/dev/vda`, `/dev/vda1`)
pub struct BlockDevInode {
    dev: Arc<Mutex<dyn BlockDevice>>,
    inode_id: u64,
}

impl BlockDevInode {
    pub fn new(dev: Arc<Mutex<dyn BlockDevice>>, inode_id: u64) -> Self {
        Self { dev, inode_id }
    }
}

impl Inode for BlockDevInode {
    fn metadata(&self) -> Metadata {
        let dev = self.dev.lock();
        let size = (dev.sector_count() * dev.sector_size() as u64) as usize;
        Metadata {
            itype: InodeType::BlockDevice,
            size,
            inode_id: self.inode_id,
            permissions: 0o660,
            uid: 0,
            gid: 0,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut dev = self.dev.lock();
        let sector_size = dev.sector_size();
        let total_bytes = (dev.sector_count() * sector_size as u64) as usize;

        if offset >= total_bytes {
            return Ok(0);
        }

        let to_read = buf.len().min(total_bytes - offset);
        let mut bytes_copied = 0usize;
        let mut scratch = [0u8; 512];

        while bytes_copied < to_read {
            let curr_offset = offset + bytes_copied;
            let sector = (curr_offset / sector_size) as u64;
            let offset_in_sec = curr_offset % sector_size;
            let chunk = (sector_size - offset_in_sec).min(to_read - bytes_copied);

            dev.read_sectors(sector, &mut scratch).map_err(|_| VfsError::IoError)?;
            buf[bytes_copied..bytes_copied + chunk]
                .copy_from_slice(&scratch[offset_in_sec..offset_in_sec + chunk]);

            bytes_copied += chunk;
        }

        Ok(bytes_copied)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut dev = self.dev.lock();
        let sector_size = dev.sector_size();
        let total_bytes = (dev.sector_count() * sector_size as u64) as usize;

        if offset >= total_bytes {
            return Err(VfsError::IoError);
        }

        let to_write = buf.len().min(total_bytes - offset);
        let mut bytes_written = 0usize;
        let mut scratch = [0u8; 512];

        while bytes_written < to_write {
            let curr_offset = offset + bytes_written;
            let sector = (curr_offset / sector_size) as u64;
            let offset_in_sec = curr_offset % sector_size;
            let chunk = (sector_size - offset_in_sec).min(to_write - bytes_written);

            if offset_in_sec != 0 || chunk < sector_size {
                // Read-Modify-Write для частичного сектора
                dev.read_sectors(sector, &mut scratch).map_err(|_| VfsError::IoError)?;
                scratch[offset_in_sec..offset_in_sec + chunk]
                    .copy_from_slice(&buf[bytes_written..bytes_written + chunk]);
                dev.write_sectors(sector, &scratch).map_err(|_| VfsError::IoError)?;
            } else {
                // Прямая запись полного сектора
                dev.write_sectors(sector, &buf[bytes_written..bytes_written + sector_size])
                    .map_err(|_| VfsError::IoError)?;
            }

            bytes_written += chunk;
        }

        Ok(bytes_written)
    }
}

/// Виртуальное блочное устройство логического раздела диска (MBR partition)
pub struct PartitionDevice {
    parent: Arc<Mutex<dyn BlockDevice>>,
    start_lba: u64,
    sector_count: u64,
}

impl PartitionDevice {
    pub fn new(parent: Arc<Mutex<dyn BlockDevice>>, start_lba: u64, sector_count: u64) -> Self {
        Self {
            parent,
            start_lba,
            sector_count,
        }
    }

    pub fn start_lba(&self) -> u64 {
        self.start_lba
    }
}

impl BlockDevice for PartitionDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let count = (buf.len() / 512) as u64;
        if start_sector + count > self.sector_count {
            return Err(BlockDeviceError::OutOfBounds);
        }
        self.parent
            .lock()
            .read_sectors(self.start_lba + start_sector, buf)
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let count = (buf.len() / 512) as u64;
        if start_sector + count > self.sector_count {
            return Err(BlockDeviceError::OutOfBounds);
        }
        self.parent
            .lock()
            .write_sectors(self.start_lba + start_sector, buf)
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MbrPartition {
    pub bootable: bool,
    pub part_type: u8,
    pub start_lba: u32,
    pub sector_count: u32,
}

/// Читает и парсит классическую таблицу разделов MBR из сектора LBA 0
pub fn parse_mbr(dev: &mut dyn BlockDevice) -> Vec<MbrPartition> {
    let mut sector0 = [0u8; 512];
    if dev.read_sectors(0, &mut sector0).is_err() {
        return Vec::new();
    }

    // Проверяем обязательную сигнатуру загрузочного сектора 0x55 0xAA
    if sector0[510] != 0x55 || sector0[511] != 0xAA {
        return Vec::new();
    }

    let mut parts = Vec::new();
    // В MBR ровно 4 первичных раздела, начиная со смещения 446 (0x1BE), каждый по 16 байт
    for i in 0..4 {
        let offset = 446 + i * 16;
        let entry = &sector0[offset..offset + 16];

        let bootable = entry[0] == 0x80;
        let part_type = entry[4];
        let start_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
        let sector_count = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);

        if part_type != 0 && sector_count > 0 {
            parts.push(MbrPartition {
                bootable,
                part_type,
                start_lba,
                sector_count,
            });
        }
    }

    parts
}

static REGISTERED_BLOCK_DEVS: RwLock<BTreeMap<String, Arc<Mutex<dyn BlockDevice>>>> =
    RwLock::new(BTreeMap::new());

pub fn register_block_device(name: String, dev: Arc<Mutex<dyn BlockDevice>>) {
    REGISTERED_BLOCK_DEVS.write().insert(name, dev);
}

pub fn get_block_device(name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
    let clean = name.trim_start_matches("/dev/");
    if let Some(dev) = REGISTERED_BLOCK_DEVS.read().get(clean).cloned() {
        return Some(dev);
    }

    // Если запрошен раздел диска (например "vda1"), а он еще не зарегистрирован
    // (например, MBR был только что записан установщиком), пересканируем разделы базового накопителя
    let mut base_end = clean.len();
    while base_end > 0 && clean.as_bytes()[base_end - 1].is_ascii_digit() {
        base_end -= 1;
    }
    if base_end > 0 && base_end < clean.len() {
        let base_name = &clean[..base_end];
        rescan_partitions(base_name);
        return REGISTERED_BLOCK_DEVS.read().get(clean).cloned();
    }

    None
}

/// Пересканирует таблицу разделов MBR на указанном накопителе (например, "vda")
/// и динамически регистрирует найденные разделы ("vda1", "vda2", ...).
pub fn rescan_partitions(base_name: &str) {
    let base_dev = match REGISTERED_BLOCK_DEVS.read().get(base_name).cloned() {
        Some(d) => d,
        None => return,
    };

    let parts = {
        let mut dev = base_dev.lock();
        parse_mbr(&mut *dev)
    };

    for (i, p) in parts.iter().enumerate() {
        let part_num = i + 1;
        let part_name = alloc::format!("{}{}", base_name, part_num);
        let is_new = !REGISTERED_BLOCK_DEVS.read().contains_key(&part_name);

        let part_dev = Arc::new(Mutex::new(PartitionDevice::new(
            base_dev.clone(),
            p.start_lba as u64,
            p.sector_count as u64,
        )));

        register_block_device(part_name.clone(), part_dev);

        if is_new {
            if let Ok(dev_dir) = crate::fs::open("/dev") {
                let _ = dev_dir.create(&part_name, InodeType::BlockDevice);
            }

            let mb = ((p.sector_count as u64) * 512) / (1024 * 1024);
            serial_println!(
                "mbr: registered /dev/{} start LBA {}, size {} MB ({} sectors), type {:#02x}",
                part_name,
                p.start_lba,
                mb,
                p.sector_count,
                p.part_type
            );
        }
    }
}

pub fn list_block_devices() -> Vec<(String, u64, usize)> {
    let guard = REGISTERED_BLOCK_DEVS.read();
    let mut list = Vec::new();
    for (name, dev) in guard.iter() {
        let d = dev.lock();
        list.push((name.clone(), d.sector_count(), d.sector_size()));
    }
    list
}

/// Инициализация блочной подсистемы VFS: регистрация всех устройств из DeviceManager (/dev/vda, /dev/sda, ...),
/// создание узлов в `/dev`, парсинг MBR и регистрация разделов.
pub fn init() {
    let dev_names =
        crate::drivers::with_device_manager(|dm| dm.block_device_names()).unwrap_or_default();
    if dev_names.is_empty() {
        serial_println!("block: no block devices found in DeviceManager");
        return;
    }

    for name in dev_names {
        let dev = match crate::drivers::get_block_device(&name) {
            Some(d) => d,
            None => continue,
        };

        register_block_device(name.clone(), dev.clone());

        // Создаем узел `/dev/<name>` в VFS
        if let Ok(dev_dir) = crate::fs::open("/dev") {
            let _ = dev_dir.create(&name, InodeType::BlockDevice);
        }

        // Парсим MBR для обнаружения разделов
        let parts = {
            let mut d = dev.lock();
            parse_mbr(&mut *d)
        };

        if parts.is_empty() {
            serial_println!("mbr: no valid partitions found on /dev/{} (raw disk)", name);
        } else {
            for (i, p) in parts.iter().enumerate() {
                let part_num = i + 1;
                let part_name = alloc::format!("{}{}", name, part_num);
                let part_dev = Arc::new(Mutex::new(PartitionDevice::new(
                    dev.clone(),
                    p.start_lba as u64,
                    p.sector_count as u64,
                )));

                register_block_device(part_name.clone(), part_dev);

                if let Ok(dev_dir) = crate::fs::open("/dev") {
                    let _ = dev_dir.create(&part_name, InodeType::BlockDevice);
                }

                let mb = ((p.sector_count as u64) * 512) / (1024 * 1024);
                serial_println!(
                    "mbr: found /dev/{} start LBA {}, size {} MB ({} sectors), type {:#02x}{}",
                    part_name,
                    p.start_lba,
                    mb,
                    p.sector_count,
                    p.part_type,
                    if p.bootable { " [bootable]" } else { "" }
                );
            }
        }
    }
}
