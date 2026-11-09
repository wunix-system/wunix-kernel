use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use super::ext4::Ext4FileSystem;
use super::fat::Fat32FileSystem;
use super::path::normalize_path;
use super::ufs::UfsFileSystem;
use super::vfs::{FileSystem, VfsError};
use crate::serial_println;

#[derive(Clone)]
pub struct MountEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub fs: Arc<dyn FileSystem>,
}

static MOUNT_TABLE: RwLock<Vec<MountEntry>> = RwLock::new(Vec::new());

/// Монтирует файловую систему раздела в указанную точку монтирования VFS
pub fn mount(source: &str, target: &str, fstype: &str, _flags: u64) -> Result<(), VfsError> {
    let norm_target = normalize_path(target);

    let (fs, detected_fstype): (Arc<dyn FileSystem>, String) = if fstype == "vboxsf" {
        // Общие папки VirtualBox обращаются напрямую к гипервизору без блочного диска
        (Arc::new(crate::drivers::virt::vbox::VBoxSfFileSystem::new(source)), String::from("vboxsf"))
    } else {
        // Ищем блочное устройство по имени источника (например, "/dev/vda1" или "vda1")
        let dev = match crate::fs::block::get_block_device(source) {
            Some(d) => d,
            None => return Err(VfsError::NotFound),
        };

        // Создаем экземпляр файловой системы по типу
        match fstype {
            "ufs" | "ufs2" => {
                let ufs = Arc::new(UfsFileSystem::open(dev)?);
                (ufs, String::from("ufs"))
            }
            "ext4" => {
                let ext4 = Arc::new(Ext4FileSystem::open(dev)?);
                (ext4, String::from("ext4"))
            }
            "fat" | "vfat" | "fat32" => {
                let fat = Arc::new(Fat32FileSystem::open(dev)?);
                (fat, String::from("fat32"))
            }
            "" | "auto" => {
                // Автодетект: проверяем нативные Unix-ФС UFS2 и Ext4, затем FAT32
                if let Ok(ufs) = UfsFileSystem::open(dev.clone()) {
                    (Arc::new(ufs), String::from("ufs"))
                } else if let Ok(ext4) = Ext4FileSystem::open(dev.clone()) {
                    (Arc::new(ext4), String::from("ext4"))
                } else if let Ok(fat) = Fat32FileSystem::open(dev) {
                    (Arc::new(fat), String::from("fat32"))
                } else {
                    return Err(VfsError::NotFound);
                }
            }
            _ => return Err(VfsError::NotSupported),
        }
    };

    // Создаем директорию точки монтирования (если это не корень)
    if norm_target != "/" {
        let _ = crate::fs::mkdir_p(&norm_target);
    }

    let mut table = MOUNT_TABLE.write();
    // Проверяем, не смонтирована ли уже эта точка
    if table.iter().any(|m| m.target == norm_target) {
        return Err(VfsError::AlreadyExists);
    }

    table.push(MountEntry {
        source: String::from(source),
        target: norm_target.clone(),
        fstype: detected_fstype.clone(),
        fs,
    });

    serial_println!("vfs: mounted {} on {} ({})", source, norm_target, detected_fstype);
    Ok(())
}

/// Отмонтирует файловую систему по точке монтирования
pub fn umount(target: &str, _flags: i32) -> Result<(), VfsError> {
    let norm_target = normalize_path(target);

    let mut table = MOUNT_TABLE.write();
    let idx = table.iter().position(|m| m.target == norm_target || m.source == norm_target);

    match idx {
        Some(i) => {
            let removed = table.remove(i);
            serial_println!("vfs: unmounted {} from {}", removed.source, removed.target);
            Ok(())
        }
        None => Err(VfsError::NotFound),
    }
}

/// Ищет смонтированную файловую систему, префикс которой совпадает с началом пути.
/// Возвращает файловую систему и относительный путь внутри неё.
pub fn find_mount(path: &str) -> Option<(Arc<dyn FileSystem>, String)> {
    let norm = normalize_path(path);
    // Системные псевдофайловые системы ядра обслуживаются в памяти (RamFS)
    if norm.starts_with("/proc") || norm.starts_with("/sys") || norm.starts_with("/dev") {
        return None;
    }

    let table = MOUNT_TABLE.read();

    let mut best_match: Option<(Arc<dyn FileSystem>, String)> = None;
    let mut longest_len = 0usize;

    for entry in table.iter() {
        let tgt = &entry.target;
        if tgt == "/" {
            if longest_len == 0 {
                let rest = norm.trim_start_matches('/');
                best_match = Some((entry.fs.clone(), String::from(rest)));
            }
            continue;
        }

        if norm == *tgt {
            return Some((entry.fs.clone(), String::new()));
        }

        if norm.starts_with(tgt) {
            let rest = &norm[tgt.len()..];
            if rest.starts_with('/') && tgt.len() > longest_len {
                longest_len = tgt.len();
                best_match = Some((entry.fs.clone(), String::from(rest.trim_start_matches('/'))));
            }
        }
    }

    best_match
}

/// Список всех активных точек монтирования для `/proc/mounts` и утилиты `mount`
pub fn list_mounts() -> Vec<MountEntry> {
    MOUNT_TABLE.read().clone()
}
