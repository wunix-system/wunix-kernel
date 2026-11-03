use crate::fs::vfs::{InodeType, VfsError};
use super::{
    read_user_cstr, resolve_relative_path, validate_user_buffer, EACCES, EEXIST, EFAULT,
    EINVAL, EISDIR, ENOENT, ENOTDIR, ENOTEMPTY,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Statfs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
}

pub fn sys_mkdir(path_ptr: u64, _mode: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&raw);
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
            let effective_mode = if _mode != 0 {
                (_mode as u16 & 0o7777) & !umask
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

pub fn sys_rmdir(path_ptr: u64) -> u64 {
    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&raw);
    if norm == "/" {
        return (-(EACCES as i64)) as u64;
    }

    let node = match crate::fs::resolve_path(&norm, false) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if node.metadata().itype != InodeType::Directory {
        return (-(ENOTDIR as i64)) as u64;
    }

    if let Ok(entries) = node.readdir() {
        let meaningful = entries
            .iter()
            .any(|e| e.name != "." && e.name != "..");
        if meaningful {
            return (-(ENOTEMPTY as i64)) as u64;
        }
    }

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

    match parent.unlink(leaf) {
        Ok(()) => 0,
        Err(_) => (-(ENOENT as i64)) as u64,
    }
}

pub fn sys_rename(oldpath_ptr: u64, newpath_ptr: u64) -> u64 {
    let old_raw = match read_user_cstr(oldpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let new_raw = match read_user_cstr(newpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let old_norm = resolve_relative_path(&old_raw);
    let new_norm = resolve_relative_path(&new_raw);
    if old_norm == new_norm {
        return 0;
    }

    let old_node = match crate::fs::resolve_path(&old_norm, false) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

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

pub fn sys_symlink(target_ptr: u64, linkpath_ptr: u64) -> u64 {
    let target = match read_user_cstr(target_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let linkpath = match read_user_cstr(linkpath_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let norm_link = resolve_relative_path(&linkpath);
    match crate::fs::create_symlink(&norm_link, &target) {
        Ok(()) => 0,
        Err(VfsError::AlreadyExists) => (-(EEXIST as i64)) as u64,
        Err(VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_readlink(path_ptr: u64, buf_ptr: u64, bufsiz: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, bufsiz) || bufsiz == 0 {
        return (-(EFAULT as i64)) as u64;
    }
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&path);
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

pub fn sys_truncate(path_ptr: u64, length: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&path);
    let node = match crate::fs::resolve_path(&norm, true) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    if node.metadata().itype == InodeType::Directory {
        return (-(EISDIR as i64)) as u64;
    }

    match node.truncate(length as usize) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_access(path_ptr: u64, mode: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&path);
    let node = match crate::fs::resolve_path(&norm, true) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    // F_OK = 0 (проверка существования)
    if mode == 0 {
        return 0;
    }

    // Для суперпользователя Wunix чтение/запись/исполнение разрешены по умолчанию
    let meta = node.metadata();
    if (mode & 1) != 0 && meta.itype == InodeType::Directory {
        return 0;
    }
    0
}

pub fn sys_statfs(_path_ptr: u64, buf_ptr: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, core::mem::size_of::<Statfs>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }

    // RAMFS_MAGIC = 0x858458f6
    let stat = Statfs {
        f_type: 0x858458f6,
        f_bsize: 4096,
        f_blocks: 131072, // 512 MiB / 4096
        f_bfree: 115200,  // ~450 MiB
        f_bavail: 115200,
        f_files: 65536,
        f_ffree: 64000,
        f_fsid: [0, 0],
        f_namelen: 255,
        f_frsize: 4096,
        f_flags: 0,
        f_spare: [0; 4],
    };

    unsafe {
        core::ptr::write_unaligned(buf_ptr as *mut Statfs, stat);
    }
    0
}
