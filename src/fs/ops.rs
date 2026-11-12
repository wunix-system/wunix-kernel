use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::block;
use crate::fs::mount;
use crate::fs::path;
use crate::fs::procfs::refresh_proc_files;
use crate::fs::vfs::{FileSystem, Inode, InodeType, VfsError};

pub static ROOT_FS: RwLock<Option<Arc<dyn FileSystem>>> = RwLock::new(None);

/// Возвращает корневой узел VFS ('/')
pub fn root() -> Result<Arc<dyn Inode>, VfsError> {
    let guard = ROOT_FS.read();
    match guard.as_ref() {
        Some(fs) => Ok(fs.root()),
        None => Err(VfsError::NotFound),
    }
}

/// Записывает файл напрямую в корневую файловую систему RamFS
pub fn write_file_ramfs(path: &str, data: &[u8]) -> Result<(), VfsError> {
    let norm = path::translate_path(path);
    let (parent_path, leaf) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => ("/", norm.as_str()),
    };

    let parent = mkdir_p_ramfs(parent_path)?;
    let node = match parent.lookup(leaf) {
        Ok(existing) => {
            let _ = existing.truncate(0);
            existing
        }
        Err(VfsError::NotFound) => parent.create(leaf, InodeType::File)?,
        Err(e) => return Err(e),
    };

    node.write_at(0, data)?;
    Ok(())
}

pub fn mkdir_p_ramfs(path: &str) -> Result<Arc<dyn Inode>, VfsError> {
    let norm = path::translate_path(path);
    let mut curr = root()?;

    for seg in norm.split('/').filter(|s| !s.is_empty() && *s != ".") {
        let next = match curr.lookup(seg) {
            Ok(node) => node,
            Err(VfsError::NotFound) => curr.create(seg, InodeType::Directory)?,
            Err(e) => return Err(e),
        };
        curr = next;
    }

    Ok(curr)
}

/// Разрешает путь в индексный узел VFS.
/// Поддерживает:
/// 1. Прозрачное перенаправление к смонтированным файловым системам (VFS Mount).
/// 2. Нормализацию и трансляцию дисков Windows (C:\ -> /win/drives/c).
/// 3. Регистронезависимый поиск в ветке /win/... (стиль NT Object Manager).
/// 4. Раскрытие символических ссылок с защитой от циклов (максимальная глубина 16).
pub fn resolve_path(path: &str, follow_last_symlink: bool) -> Result<Arc<dyn Inode>, VfsError> {
    let mut current_path = path::translate_path(path);
    let mut symlink_depth = 0;

    'resolve_loop: loop {
        if symlink_depth > 16 {
            return Err(VfsError::InvalidPath);
        }

        let norm = path::normalize_path(&current_path);

        if norm == "/proc/self/exe" || norm == "/proc/thread-self/exe" {
            current_path = crate::task::current_thread_exe_path();
            symlink_depth += 1;
            continue 'resolve_loop;
        }

        if norm.starts_with("/proc/") || norm.starts_with("/sys/") || norm == "/etc/mtab" || norm == "/etc/resolv.conf" {
            refresh_proc_files(&norm);
        }

        // Прямое сопоставление блочных устройств (/dev/vda, /dev/vda1) с их драйверами
        if norm.starts_with("/dev/") {
            let dev_name = norm.trim_start_matches("/dev/");
            if let Some(blk_dev) = block::get_block_device(dev_name) {
                return Ok(Arc::new(block::BlockDevInode::new(blk_dev, 0xDE00)));
            }
        }

        // Проверяем таблицу монтирования: если путь обращается к смонтированной ФС (например, /mnt/...)
        if let Some((m_fs, subpath)) = mount::find_mount(&norm) {
            if subpath.is_empty() {
                return Ok(m_fs.root());
            }
            let sub_segments: Vec<&str> = subpath
                .split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect();
            let mut m_curr = m_fs.root();
            for seg in sub_segments {
                m_curr = m_curr.lookup(seg)?;
            }
            return Ok(m_curr);
        }

        let segments: Vec<&str> = current_path
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();

        let mut curr = root()?;
        let is_win = path::is_windows_subsystem(&current_path);

        for (idx, &seg) in segments.iter().enumerate() {
            let is_last = idx == segments.len() - 1;

            let next_node = match curr.lookup(seg) {
                Ok(node) => node,
                Err(VfsError::NotFound) if is_win => {
                    // Регистронезависимый fallback в пространстве /win
                    let entries = curr.readdir()?;
                    if let Some(entry) = entries.iter().find(|e| e.name.eq_ignore_ascii_case(seg)) {
                        curr.lookup(&entry.name)?
                    } else {
                        return Err(VfsError::NotFound);
                    }
                }
                Err(e) => return Err(e),
            };

            let meta = next_node.metadata();
            if meta.itype == InodeType::Symlink && (!is_last || follow_last_symlink) {
                let target = next_node.read_link()?;
                symlink_depth += 1;

                let remaining = &segments[idx + 1..];
                let mut new_path = if target.starts_with('/') {
                    target
                } else {
                    let parent_segments = &segments[..idx];
                    let mut p = String::new();
                    for ps in parent_segments {
                        p.push('/');
                        p.push_str(ps);
                    }
                    p.push('/');
                    p.push_str(&target);
                    p
                };

                for rem in remaining {
                    new_path.push('/');
                    new_path.push_str(rem);
                }

                current_path = path::normalize_path(&new_path);
                continue 'resolve_loop;
            }

            curr = next_node;
        }

        return Ok(curr);
    }
}

/// Открывает индексный узел по указанному пути, разрешая символические ссылки
pub fn open(path: &str) -> Result<Arc<dyn Inode>, VfsError> {
    resolve_path(path, true)
}

/// Рекурсивно создает цепочку директорий вдоль пути
pub fn mkdir_p(path: &str) -> Result<Arc<dyn Inode>, VfsError> {
    let norm = path::normalize_path(&path::translate_path(path));
    if let Some((m_fs, subpath)) = mount::find_mount(&norm) {
        let mut curr = m_fs.root();
        for seg in subpath.split('/').filter(|s| !s.is_empty() && *s != ".") {
            let next = match curr.lookup(seg) {
                Ok(node) => node,
                Err(VfsError::NotFound) => curr.create(seg, InodeType::Directory)?,
                Err(e) => return Err(e),
            };
            curr = next;
        }
        return Ok(curr);
    }

    let mut curr = root()?;
    for seg in norm.split('/').filter(|s| !s.is_empty() && *s != ".") {
        let next = match curr.lookup(seg) {
            Ok(node) => node,
            Err(VfsError::NotFound) => curr.create(seg, InodeType::Directory)?,
            Err(e) => return Err(e),
        };
        curr = next;
    }

    Ok(curr)
}

/// Создает символическую ссылку
pub fn create_symlink(path: &str, target: &str) -> Result<(), VfsError> {
    let norm = path::translate_path(path);
    let (parent_path, leaf) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => ("/", norm.as_str()),
    };

    let parent = mkdir_p(parent_path)?;
    let link_node = match parent.lookup(leaf) {
        Ok(node) => node,
        Err(VfsError::NotFound) => parent.create(leaf, InodeType::Symlink)?,
        Err(e) => return Err(e),
    };

    link_node.write_at(0, target.as_bytes())?;
    Ok(())
}

/// Считывает всё содержимое файла в вектор байтов
pub fn read_file(path: &str) -> Result<Vec<u8>, VfsError> {
    let node = resolve_path(path, true)?;
    let meta = node.metadata();
    if meta.itype == InodeType::Directory {
        return Err(VfsError::IsADirectory);
    }

    let mut buf = alloc::vec![0u8; meta.size];
    let bytes_read = node.read_at(0, &mut buf)?;
    buf.truncate(bytes_read);
    Ok(buf)
}

/// Записывает байты в файл по указанному пути.
/// Если файл или промежуточные директории отсутствуют, создает их.
pub fn write_file(path: &str, data: &[u8]) -> Result<(), VfsError> {
    let norm = path::normalize_path(&path::translate_path(path));
    if norm == "/" {
        return Err(VfsError::IsADirectory);
    }

    if let Some((m_fs, subpath)) = mount::find_mount(&norm) {
        let (parent_path, leaf) = match subpath.rfind('/') {
            Some(idx) => (&subpath[..idx], &subpath[idx + 1..]),
            None => ("", subpath.as_str()),
        };
        let mut parent = m_fs.root();
        for seg in parent_path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            parent = match parent.lookup(seg) {
                Ok(node) => node,
                Err(VfsError::NotFound) => parent.create(seg, InodeType::Directory)?,
                Err(e) => return Err(e),
            };
        }
        let node = match parent.lookup(leaf) {
            Ok(existing) => existing,
            Err(VfsError::NotFound) => parent.create(leaf, InodeType::File)?,
            Err(e) => return Err(e),
        };

        if node.metadata().itype == InodeType::Directory {
            return Err(VfsError::IsADirectory);
        }

        node.write_at(0, data)?;
        return Ok(());
    }

    let (parent_path, leaf) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => ("/", norm.as_str()),
    };

    let parent = mkdir_p(parent_path)?;
    let node = match parent.lookup(leaf) {
        Ok(existing) => existing,
        Err(VfsError::NotFound) => parent.create(leaf, InodeType::File)?,
        Err(e) => return Err(e),
    };

    if node.metadata().itype == InodeType::Directory {
        return Err(VfsError::IsADirectory);
    }

    node.write_at(0, data)?;
    Ok(())
}

/// Удаляет файл или символическую ссылку по указанному пути
pub fn unlink(path: &str) -> Result<(), VfsError> {
    let norm = path::normalize_path(&path::translate_path(path));
    if norm == "/" {
        return Err(VfsError::IsADirectory);
    }

    if let Some((m_fs, subpath)) = mount::find_mount(&norm) {
        let (parent_path, leaf) = match subpath.rfind('/') {
            Some(idx) => (&subpath[..idx], &subpath[idx + 1..]),
            None => ("", subpath.as_str()),
        };
        let mut parent = m_fs.root();
        for seg in parent_path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            parent = parent.lookup(seg)?;
        }
        return parent.unlink(leaf);
    }

    let (parent_path, leaf) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => ("/", norm.as_str()),
    };

    let parent = resolve_path(parent_path, true)?;
    parent.unlink(leaf)
}

/// Проверяет права текущего потока ядра на доступ к узлу VFS по стандартам POSIX.
/// Root (EUID == 0) всегда имеет права на чтение и запись, а на исполнение —
/// если хотя бы один бит 'x' (владелец, группа или остальные) выставлен в режиме файла.
pub fn check_permission(
    node: &Arc<dyn Inode>,
    want_read: bool,
    want_write: bool,
    want_exec: bool,
) -> Result<(), VfsError> {
    let meta = node.metadata();
    let euid = crate::task::current_thread_euid();
    let egid = crate::task::current_thread_egid();

    if euid == 0 {
        if want_exec && (meta.permissions & 0o111) == 0 {
            return Err(VfsError::PermissionDenied);
        }
        return Ok(());
    }

    let bits = if euid == meta.uid {
        (meta.permissions >> 6) & 7
    } else if egid == meta.gid {
        (meta.permissions >> 3) & 7
    } else {
        meta.permissions & 7
    };

    if want_read && (bits & 4) == 0 {
        return Err(VfsError::PermissionDenied);
    }
    if want_write && (bits & 2) == 0 {
        return Err(VfsError::PermissionDenied);
    }
    if want_exec && (bits & 1) == 0 {
        return Err(VfsError::PermissionDenied);
    }

    Ok(())
}
