use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{
    get_file_handle, insert_file_handle, InodeType, RegularFileHandle, VfsError, O_CREAT,
};
use super::{
    read_user_cstr, resolve_relative_path, validate_user_buffer, EACCES, EBADF, EEXIST, EFAULT,
    EINVAL, ENOENT, ENOTDIR, EPERM, Stat,
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

    // Псевдоустройства терминалов и нулевые устройства Linux
    if target_path == "/dev/tty" || target_path == "/dev/tty0" || target_path == "/dev/console" {
        let handle = Arc::new(crate::fs::handle::TtyHandle::new(flags as u32));
        return insert_file_handle(handle);
    }
    if target_path == "/dev/null" {
        let handle = Arc::new(crate::fs::handle::NullHandle::new(flags as u32));
        return insert_file_handle(handle);
    }
    if target_path == "/dev/zero" {
        let handle = Arc::new(crate::fs::handle::ZeroHandle::new(flags as u32));
        return insert_file_handle(handle);
    }
    if target_path == "/dev/urandom" || target_path == "/dev/random" {
        let handle = Arc::new(crate::fs::handle::UrandomHandle::new(flags as u32));
        return insert_file_handle(handle);
    }

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

    let (is_special_dev, dev_mode, dev_rdev) = match target_path.as_str() {
        "/dev/tty" | "/dev/tty0" | "/dev/console" => (true, 0o020666, 0x0500),
        "/dev/null" => (true, 0o020666, 0x0103),
        "/dev/zero" => (true, 0o020666, 0x0105),
        "/dev/urandom" => (true, 0o020666, 0x0109),
        "/dev/random" => (true, 0o020666, 0x0108),
        _ => (false, 0, 0),
    };

    if is_special_dev {
        let mut stat = Stat::default();
        stat.st_mode = dev_mode;
        stat.st_rdev = dev_rdev;
        stat.st_nlink = 1;
        stat.st_blksize = 4096;
        unsafe {
            core::ptr::write_unaligned(statbuf_ptr as *mut Stat, stat);
        }
        return 0;
    }

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

pub fn sys_renameat2(olddirfd: i64, oldpath_ptr: u64, newdirfd: i64, newpath_ptr: u64, flags: u64) -> u64 {
    const RENAME_NOREPLACE: u64 = 1;

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

    if old_norm == new_norm {
        return 0;
    }

    let old_node = match crate::fs::resolve_path(&old_norm, false) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if (flags & RENAME_NOREPLACE) != 0 && crate::fs::resolve_path(&new_norm, false).is_ok() {
        return (-(EEXIST as i64)) as u64;
    }

    let (old_parent_path, old_leaf) = match old_norm.rfind('/') {
        Some(0) => ("/", &old_norm[1..]),
        Some(idx) => (&old_norm[..idx], &old_norm[idx + 1..]),
        None => ("/", old_norm.as_str()),
    };
    let (new_parent_path, new_leaf) = match new_norm.rfind('/') {
        Some(0) => ("/", &new_norm[1..]),
        Some(idx) => (&new_norm[..idx], &new_norm[idx + 1..]),
        None => ("/", new_norm.as_str()),
    };

    let old_parent = match crate::fs::resolve_path(old_parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };
    let new_parent = match crate::fs::resolve_path(new_parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let _ = new_parent.unlink(new_leaf);
    if new_parent.insert_child(new_leaf, old_node).is_err() {
        return (-(EINVAL as i64)) as u64;
    }
    let _ = old_parent.unlink(old_leaf);

    0
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
