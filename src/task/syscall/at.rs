use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{
    get_file_handle, insert_file_handle, InodeType, RegularFileHandle, VfsError, O_CREAT,
};
use super::{
    read_user_cstr, resolve_relative_path, validate_user_buffer, EACCES, EBADF, EBUSY, EEXIST,
    EFAULT, EINVAL, EISDIR, ENOENT, ENOTDIR, ENOTEMPTY, EPERM, EXDEV, Stat,
};

pub const AT_FDCWD: i64 = -100;
pub const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
pub const AT_REMOVEDIR: u64 = 0x200;
pub const AT_SYMLINK_FOLLOW: u64 = 0x400;
pub const AT_EMPTY_PATH: u64 = 0x1000;

/// Разрешает относительный путь пользователя с учетом дескриптора базового каталога dirfd.
/// Стандарт POSIX / Linux допускает спецзначение AT_FDCWD (-100) для разрешения от CWD процесса.
pub fn resolve_at_path(dirfd: i64, path: &str, allow_empty: bool) -> Result<String, u64> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        if allow_empty && dirfd >= 0 {
            let handle = get_file_handle(dirfd as u64).ok_or(EBADF)?;
            return handle.path().ok_or(EBADF);
        }
        return Err(ENOENT);
    }

    if trimmed.starts_with('/') || crate::fs::path::is_windows_subsystem(trimmed) {
        return Ok(crate::fs::path::normalize_path(trimmed));
    }

    if dirfd == AT_FDCWD {
        return Ok(resolve_relative_path(trimmed));
    }

    if dirfd < 0 {
        return Err(EBADF);
    }

    let handle = get_file_handle(dirfd as u64).ok_or(EBADF)?;
    if let Some(inode) = handle.as_inode() {
        if inode.metadata().itype != InodeType::Directory {
            return Err(ENOTDIR);
        }
    } else {
        return Err(EBADF);
    }

    let base_path = handle.path().unwrap_or_else(super::get_cwd);
    let combined = if base_path == "/" {
        format!("/{}", trimmed)
    } else {
        format!("{}/{}", base_path, trimmed)
    };

    Ok(crate::fs::path::normalize_path(&combined))
}

pub fn sys_openat(dirfd: i64, path_ptr: u64, flags: u64, mode: u64) -> u64 {
    let raw_path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = match resolve_at_path(dirfd, &raw_path, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let want_read = (flags & 3) == 0 || (flags & 3) == 2;
    let want_write = (flags & 3) == 1 || (flags & 3) == 2 || (flags & 0x200) != 0;

    let node = match crate::fs::open(&target_path) {
        Ok(n) => {
            if let Err(_) = crate::fs::check_permission(&n, want_read, want_write, false) {
                return (-(EACCES as i64)) as u64;
            }
            if (flags & 0x200) != 0 {
                let _ = n.truncate(0);
            }
            n
        }
        Err(_) => {
            if (flags & (O_CREAT as u64)) != 0 {
                let (parent_path, leaf) = match target_path.rfind('/') {
                    Some(0) => ("/", &target_path[1..]),
                    Some(idx) => (&target_path[..idx], &target_path[idx + 1..]),
                    None => (".", target_path.as_str()),
                };

                let parent_node = match crate::fs::resolve_path(parent_path, true) {
                    Ok(p) => p,
                    Err(_) => return (-(ENOENT as i64)) as u64,
                };

                if let Err(_) = crate::fs::check_permission(&parent_node, false, true, true) {
                    return (-(EACCES as i64)) as u64;
                }

                let new_file = match parent_node.create(leaf, InodeType::File) {
                    Ok(f) => f,
                    Err(_) => return (-(EACCES as i64)) as u64,
                };

                let effective_mode = if mode != 0 {
                    (mode as u16 & 0o7777) & !(crate::task::current_thread_umask() as u16)
                } else {
                    0o666 & !(crate::task::current_thread_umask() as u16)
                };
                let _ = new_file.chmod(effective_mode);
                let _ = new_file.chown(crate::task::current_thread_euid(), crate::task::current_thread_egid());

                new_file
            } else {
                return (-(ENOENT as i64)) as u64;
            }
        }
    };

    if let Some(dev_handle) = crate::fs::devfs::open_device_node(&node, flags as u32, Some(target_path.clone())) {
        return insert_file_handle(dev_handle);
    }

    let initial_offset = if (flags & 0x400) != 0 {
        node.metadata().size
    } else {
        0
    };

    let handle = Arc::new(RegularFileHandle {
        inode: node,
        offset: Mutex::new(initial_offset),
        flags: core::sync::atomic::AtomicU32::new(flags as u32),
        path: Some(target_path),
    });

    insert_file_handle(handle)
}

pub fn sys_newfstatat(dirfd: i64, path_ptr: u64, statbuf_ptr: u64, flags: u64) -> u64 {
    if !validate_user_buffer(statbuf_ptr, core::mem::size_of::<Stat>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let raw_path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = match resolve_at_path(dirfd, &raw_path, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    let node = match crate::fs::resolve_path(&target_path, follow_symlinks) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let meta = node.metadata();
    let mut stat = Stat::default();
    stat.st_size = meta.size as i64;
    stat.st_ino = meta.inode_id;
    stat.st_nlink = 1;
    stat.st_blksize = 4096;
    let type_bits = match meta.itype {
        InodeType::Directory => 0o040000,
        InodeType::File => 0o100000,
        InodeType::Symlink => 0o120000,
        InodeType::BlockDevice => 0o060000,
        InodeType::CharDevice => 0o020000,
    };
    stat.st_mode = type_bits | (meta.permissions as u32 & 0o7777);
    stat.st_rdev = meta.rdev;
    stat.st_uid = meta.uid;
    stat.st_gid = meta.gid;

    unsafe {
        core::ptr::write_unaligned(statbuf_ptr as *mut Stat, stat);
    }
    0
}

pub fn sys_mkdirat(dirfd: i64, path_ptr: u64, mode: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = match resolve_at_path(dirfd, &raw, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    let (parent_path, leaf) = match norm.rfind('/') {
        Some(0) => ("/", &norm[1..]),
        Some(idx) => (&norm[..idx], &norm[idx + 1..]),
        None => ("/", norm.as_str()),
    };

    let parent = match crate::fs::resolve_path(parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if let Err(_) = crate::fs::check_permission(&parent, false, true, true) {
        return (-(EACCES as i64)) as u64;
    }

    match parent.create(leaf, InodeType::Directory) {
        Ok(dir_node) => {
            let umask = crate::task::current_thread_umask() as u16;
            let effective_mode = if mode != 0 {
                (mode as u16 & 0o7777) & !umask
            } else {
                0o777 & !umask
            };
            let _ = dir_node.chmod(effective_mode);
            let _ = dir_node.chown(crate::task::current_thread_euid(), crate::task::current_thread_egid());
            0
        }
        Err(VfsError::AlreadyExists) => (-(EEXIST as i64)) as u64,
        Err(VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(VfsError::NotADirectory) => (-(ENOTDIR as i64)) as u64,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_unlinkat(dirfd: i64, path_ptr: u64, flags: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = match resolve_at_path(dirfd, &raw, false) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    if (flags & AT_REMOVEDIR) != 0 {
        return super::dir::sys_rmdir_path(&target_path);
    }

    let (parent_path, leaf) = match target_path.rfind('/') {
        Some(0) => ("/", &target_path[1..]),
        Some(idx) => (&target_path[..idx], &target_path[idx + 1..]),
        None => (".", target_path.as_str()),
    };

    let parent_node = match crate::fs::resolve_path(parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if let Err(_) = crate::fs::check_permission(&parent_node, false, true, true) {
        return (-(EACCES as i64)) as u64;
    }

    let file_node = match parent_node.lookup(leaf) {
        Ok(f) => f,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let parent_meta = parent_node.metadata();
    if (parent_meta.permissions & 0o1000) != 0 {
        let file_meta = file_node.metadata();
        let euid = crate::task::current_thread_euid();
        if euid != 0 && euid != file_meta.uid && euid != parent_meta.uid {
            return (-(EPERM as i64)) as u64;
        }
    }

    match parent_node.unlink(leaf) {
        Ok(()) => 0,
        Err(VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(VfsError::IsADirectory) => (-(super::EISDIR as i64)) as u64,
        Err(VfsError::PermissionDenied) => (-(EACCES as i64)) as u64,
        Err(_) => (-(super::EIO as i64)) as u64,
    }
}

pub fn sys_renameat(olddirfd: i64, oldpath_ptr: u64, newdirfd: i64, newpath_ptr: u64) -> u64 {
    sys_renameat2(olddirfd, oldpath_ptr, newdirfd, newpath_ptr, 0)
}

fn split_parent_leaf(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(idx) => (&path[..idx], &path[idx + 1..]),
        None => ("/", path),
    }
}

fn is_subpath(parent: &str, candidate: &str) -> bool {
    if parent == "/" {
        return candidate != "/";
    }
    candidate.len() > parent.len()
        && candidate.starts_with(parent)
        && candidate.as_bytes()[parent.len()] == b'/'
}

fn same_mount(old_path: &str, new_path: &str) -> bool {
    match (
        crate::fs::mount::find_mount(old_path),
        crate::fs::mount::find_mount(new_path),
    ) {
        (None, None) => true,
        (Some((fs1, _)), Some((fs2, _))) => Arc::ptr_eq(&fs1, &fs2),
        _ => false,
    }
}

fn validate_rename_target(
    old_type: InodeType,
    new_norm: &str,
    noreplace: bool,
) -> Result<(), u64> {
    let Ok(new_node) = crate::fs::resolve_path(new_norm, false) else {
        return Ok(());
    };
    if noreplace {
        return Err(EEXIST);
    }
    let new_type = new_node.metadata().itype;
    if old_type == InodeType::Directory && new_type != InodeType::Directory {
        return Err(ENOTDIR);
    }
    if old_type != InodeType::Directory && new_type == InodeType::Directory {
        return Err(EISDIR);
    }
    if new_type == InodeType::Directory {
        let entries = new_node.readdir().map_err(|_| ENOTEMPTY)?;
        if entries.iter().any(|e| e.name != "." && e.name != "..") {
            return Err(ENOTEMPTY);
        }
    }
    Ok(())
}

pub fn sys_renameat2(olddirfd: i64, oldpath_ptr: u64, newdirfd: i64, newpath_ptr: u64, flags: u64) -> u64 {
    const RENAME_NOREPLACE: u64 = 1;

    if (flags & !RENAME_NOREPLACE) != 0 {
        return (-(EINVAL as i64)) as u64;
    }

    let old_raw = match read_user_cstr(oldpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let new_raw = match read_user_cstr(newpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let old_norm = match resolve_at_path(olddirfd, &old_raw, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };
    let new_norm = match resolve_at_path(newdirfd, &new_raw, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    let old_node = match crate::fs::resolve_path(&old_norm, false) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if old_norm == new_norm {
        return 0;
    }
    if old_norm == "/" || new_norm == "/" {
        return (-(EBUSY as i64)) as u64;
    }
    if old_node.metadata().itype == InodeType::Directory && is_subpath(&old_norm, &new_norm) {
        return (-(EINVAL as i64)) as u64;
    }

    let (old_parent_path, old_leaf) = split_parent_leaf(&old_norm);
    let (new_parent_path, new_leaf) = split_parent_leaf(&new_norm);
    if old_leaf.is_empty() || new_leaf.is_empty() || old_leaf == "." || old_leaf == ".." || new_leaf == "." || new_leaf == ".." {
        return (-(EINVAL as i64)) as u64;
    }

    let old_parent = match crate::fs::resolve_path(old_parent_path, true) {
        Ok(p) if p.metadata().itype == InodeType::Directory => p,
        Ok(_) => return (-(ENOTDIR as i64)) as u64,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };
    let new_parent = match crate::fs::resolve_path(new_parent_path, true) {
        Ok(p) if p.metadata().itype == InodeType::Directory => p,
        Ok(_) => return (-(ENOTDIR as i64)) as u64,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if let Err(e) = validate_rename_target(old_node.metadata().itype, &new_norm, (flags & RENAME_NOREPLACE) != 0) {
        return (-(e as i64)) as u64;
    }

    if !same_mount(&old_norm, &new_norm) {
        return (-(EXDEV as i64)) as u64;
    }

    // Если ФС (например, Ext4/FAT32/UFS) не реализует insert_child на уровне VFS,
    // возвращаем EXDEV до изменения целевого узла — mv выполнит перенос через copy + unlink.
    match new_parent.insert_child(new_leaf, old_node) {
        Ok(()) => {
            let _ = old_parent.unlink(old_leaf);
            0
        }
        Err(VfsError::NotADirectory) | Err(VfsError::NotSupported) => (-(EXDEV as i64)) as u64,
        Err(VfsError::PermissionDenied) => (-(EACCES as i64)) as u64,
        Err(_) => (-(EINVAL as i64)) as u64,
    }
}

pub fn sys_fchmodat(dirfd: i64, path_ptr: u64, mode: u64, flags: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = match resolve_at_path(dirfd, &raw, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };
    let follow = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    let node = match crate::fs::resolve_path(&norm, follow) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let meta = node.metadata();
    let euid = crate::task::current_thread_euid();
    if euid != 0 && euid != meta.uid {
        return (-(EPERM as i64)) as u64;
    }

    match node.chmod((mode & 0o7777) as u16) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_fchownat(dirfd: i64, path_ptr: u64, uid: u64, gid: u64, flags: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = match resolve_at_path(dirfd, &raw, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };
    let follow = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    let node = match crate::fs::resolve_path(&norm, follow) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let euid = crate::task::current_thread_euid();
    if euid != 0 {
        return (-(EPERM as i64)) as u64;
    }

    match node.chown(uid as u32, gid as u32) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_readlinkat(dirfd: i64, path_ptr: u64, buf_ptr: u64, bufsiz: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, bufsiz) || bufsiz == 0 {
        return (-(EFAULT as i64)) as u64;
    }
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = match resolve_at_path(dirfd, &raw, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    if norm == "/proc/self/exe" || norm == "/proc/thread-self/exe" || (norm.starts_with("/proc/") && norm.ends_with("/exe")) {
        let exe = crate::task::current_thread_exe_path();
        let bytes = exe.as_bytes();
        let to_copy = bytes.len().min(bufsiz as usize);
        unsafe {
            let dst = buf_ptr as *mut u8;
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, to_copy);
        }
        return to_copy as u64;
    }

    let node = match crate::fs::resolve_path(&norm, false) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let target = match node.read_link() {
        Ok(t) => t,
        Err(_) => return (-(EINVAL as i64)) as u64,
    };

    let bytes = target.as_bytes();
    let to_copy = bytes.len().min(bufsiz as usize);
    unsafe {
        let dst = buf_ptr as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, to_copy);
    }

    to_copy as u64
}

pub fn sys_symlinkat(target_ptr: u64, newdirfd: i64, linkpath_ptr: u64) -> u64 {
    let target = match read_user_cstr(target_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let raw_link = match read_user_cstr(linkpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let norm_link = match resolve_at_path(newdirfd, &raw_link, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    match crate::fs::create_symlink(&norm_link, &target) {
        Ok(()) => 0,
        Err(VfsError::AlreadyExists) => (-(EEXIST as i64)) as u64,
        Err(VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_faccessat(dirfd: i64, path_ptr: u64, mode: u64, flags: u64) -> u64 {
    sys_faccessat2(dirfd, path_ptr, mode, flags)
}

pub fn sys_faccessat2(dirfd: i64, path_ptr: u64, mode: u64, flags: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = match resolve_at_path(dirfd, &raw, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };
    let follow = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    let node = match crate::fs::resolve_path(&norm, follow) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    // F_OK = 0 (проверка существования)
    if mode == 0 {
        return 0;
    }

    let want_read = (mode & 4) != 0;
    let want_write = (mode & 2) != 0;
    let want_exec = (mode & 1) != 0;

    match crate::fs::check_permission(&node, want_read, want_write, want_exec) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_mknodat(dirfd: i64, path_ptr: u64, mode: u64, dev: u64) -> u64 {
    let raw_path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = match resolve_at_path(dirfd, &raw_path, false) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let (parent_path, leaf) = match target_path.rfind('/') {
        Some(0) => ("/", &target_path[1..]),
        Some(idx) => (&target_path[..idx], &target_path[idx + 1..]),
        None => (".", target_path.as_str()),
    };

    let parent_node = match crate::fs::resolve_path(parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if let Err(_) = crate::fs::check_permission(&parent_node, false, true, true) {
        return (-(EACCES as i64)) as u64;
    }

    let file_type = mode & 0o170000;
    let permissions = (mode as u16 & 0o7777) & !(crate::task::current_thread_umask() as u16);

    let new_node: Arc<dyn crate::fs::Inode> = if file_type == 0o020000 {
        // S_IFCHR: символьное устройство
        Arc::new(crate::fs::devfs::DevInode::new_char(
            dev,
            permissions,
            Arc::new(crate::fs::devfs::mem::NullDev),
        ))
    } else if file_type == 0o060000 {
        // S_IFBLK: блочное устройство
        let loop_dev = crate::fs::devfs::loopdev::create_loop_device(99);
        Arc::new(crate::fs::devfs::DevInode::new_block(dev, permissions, loop_dev))
    } else if file_type == 0o010000 {
        // S_IFIFO: именованный канал
        Arc::new(crate::fs::RamInode::new_file())
    } else {
        // S_IFREG: обычный файл
        match parent_node.create(leaf, crate::fs::InodeType::File) {
            Ok(f) => {
                let _ = f.chmod(permissions);
                return 0;
            }
            Err(_) => return (-(super::EIO as i64)) as u64,
        }
    };

    match parent_node.insert_child(leaf, new_node) {
        Ok(_) => 0,
        Err(_) => (-(super::EIO as i64)) as u64,
    }
}

pub fn sys_linkat(olddirfd: i64, oldpath_ptr: u64, newdirfd: i64, newpath_ptr: u64, flags: u64) -> u64 {
    let old_raw = match read_user_cstr(oldpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let new_raw = match read_user_cstr(newpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let old_norm = match resolve_at_path(olddirfd, &old_raw, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };
    let new_norm = match resolve_at_path(newdirfd, &new_raw, false) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    let follow = (flags & AT_SYMLINK_FOLLOW) != 0;
    let old_node = match crate::fs::resolve_path(&old_norm, follow) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if old_node.metadata().itype == InodeType::Directory {
        return (-(EPERM as i64)) as u64;
    }

    if crate::fs::resolve_path(&new_norm, false).is_ok() {
        return (-(EEXIST as i64)) as u64;
    }

    let (new_parent_path, new_leaf) = match new_norm.rfind('/') {
        Some(0) => ("/", &new_norm[1..]),
        Some(idx) => (&new_norm[..idx], &new_norm[idx + 1..]),
        None => ("/", new_norm.as_str()),
    };

    let new_parent = match crate::fs::resolve_path(new_parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if let Err(_) = crate::fs::check_permission(&new_parent, false, true, true) {
        return (-(EACCES as i64)) as u64;
    }

    match new_parent.insert_child(new_leaf, old_node) {
        Ok(()) => 0,
        Err(VfsError::AlreadyExists) => (-(EEXIST as i64)) as u64,
        Err(VfsError::NotADirectory) => (-(ENOTDIR as i64)) as u64,
        Err(_) => (-(EPERM as i64)) as u64,
    }
}
