use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{
    get_file_handle, insert_file_handle, remove_file_handle, InodeType, RegularFileHandle,
};
use super::{
    read_user_cstr, validate_user_buffer, EACCES, EBADF, EFAULT, EINVAL, ENOENT, EOPNOTSUPP,
};
use super::at::{resolve_at_path, AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub __spare2: [u64; 12],
}

pub const STATX_TYPE: u32 = 0x0001;
pub const STATX_MODE: u32 = 0x0002;
pub const STATX_NLINK: u32 = 0x0004;
pub const STATX_UID: u32 = 0x0008;
pub const STATX_GID: u32 = 0x0010;
pub const STATX_ATIME: u32 = 0x0020;
pub const STATX_MTIME: u32 = 0x0040;
pub const STATX_CTIME: u32 = 0x0080;
pub const STATX_INO: u32 = 0x0100;
pub const STATX_SIZE: u32 = 0x0200;
pub const STATX_BLOCKS: u32 = 0x0400;
pub const STATX_BASIC_STATS: u32 = 0x07ff;

pub fn sys_statx(dirfd: i64, path_ptr: u64, flags: u64, mask: u64, statxbuf_ptr: u64) -> u64 {
    if !validate_user_buffer(statxbuf_ptr, core::mem::size_of::<Statx>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }

    let raw = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };

    let target_path = match resolve_at_path(dirfd, &raw, (flags & AT_EMPTY_PATH) != 0) {
        Ok(p) => p,
        Err(e) => return (-(e as i64)) as u64,
    };

    let follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    let node = match crate::fs::resolve_path(&target_path, follow_symlinks) {
        Ok(n) => n,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let meta = node.metadata();
    let type_bits = match meta.itype {
        InodeType::Directory => 0o040000,
        InodeType::File => 0o100000,
        InodeType::Symlink => 0o120000,
        InodeType::BlockDevice => 0o060000,
        InodeType::CharDevice => 0o020000,
    };

    let ticks = crate::arch::x86_64::timer::ticks();
    let now_sec = (ticks / 100) as i64;
    let now_nsec = ((ticks % 100) * 10_000_000) as u32;

    let mut stx = Statx::default();
    stx.stx_mask = (mask as u32) & STATX_BASIC_STATS;
    stx.stx_blksize = 4096;
    stx.stx_nlink = 1;
    stx.stx_uid = meta.uid;
    stx.stx_gid = meta.gid;
    stx.stx_mode = (type_bits | (meta.permissions as u32 & 0o7777)) as u16;
    stx.stx_ino = meta.inode_id;
    stx.stx_size = meta.size as u64;
    stx.stx_blocks = ((meta.size as u64) + 511) / 512;
    stx.stx_atime = StatxTimestamp { tv_sec: now_sec, tv_nsec: now_nsec, __reserved: 0 };
    stx.stx_mtime = StatxTimestamp { tv_sec: now_sec, tv_nsec: now_nsec, __reserved: 0 };
    stx.stx_ctime = StatxTimestamp { tv_sec: now_sec, tv_nsec: now_nsec, __reserved: 0 };
    stx.stx_btime = StatxTimestamp { tv_sec: now_sec, tv_nsec: now_nsec, __reserved: 0 };

    unsafe {
        core::ptr::write_unaligned(statxbuf_ptr as *mut Statx, stx);
    }

    0
}

/// Выполняет копирование байт напрямую в ядре без промежуточных переходов в Ring 3.
pub fn sys_copy_file_range(
    fd_in: u64,
    off_in_ptr: u64,
    fd_out: u64,
    off_out_ptr: u64,
    len: u64,
    _flags: u64,
) -> u64 {
    if len == 0 {
        return 0;
    }

    let handle_in = match get_file_handle(fd_in) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let handle_out = match get_file_handle(fd_out) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };

    let inode_in = match handle_in.as_inode() {
        Some(i) => i,
        None => return (-(EINVAL as i64)) as u64,
    };
    let inode_out = match handle_out.as_inode() {
        Some(i) => i,
        None => return (-(EINVAL as i64)) as u64,
    };

    let mut cur_off_in = if off_in_ptr != 0 {
        if !validate_user_buffer(off_in_ptr, 8) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { *(off_in_ptr as *const u64) as usize }
    } else {
        handle_in.offset()
    };

    let mut cur_off_out = if off_out_ptr != 0 {
        if !validate_user_buffer(off_out_ptr, 8) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { *(off_out_ptr as *const u64) as usize }
    } else {
        handle_out.offset()
    };

    let mut copied = 0u64;
    let mut chunk = [0u8; 4096];

    while copied < len {
        let to_read = ((len - copied) as usize).min(chunk.len());
        let nread = match inode_in.read_at(cur_off_in, &mut chunk[..to_read]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                if copied > 0 {
                    break;
                }
                return (-(super::EIO as i64)) as u64;
            }
        };

        let nwritten = match inode_out.write_at(cur_off_out, &chunk[..nread]) {
            Ok(n) => n,
            Err(_) => {
                if copied > 0 {
                    break;
                }
                return (-(super::EIO as i64)) as u64;
            }
        };

        cur_off_in += nwritten;
        cur_off_out += nwritten;
        copied += nwritten as u64;

        if nwritten < nread {
            break;
        }
    }

    if off_in_ptr != 0 {
        unsafe { *(off_in_ptr as *mut u64) = cur_off_in as u64; }
    } else {
        handle_in.set_offset(cur_off_in);
    }

    if off_out_ptr != 0 {
        unsafe { *(off_out_ptr as *mut u64) = cur_off_out as u64; }
    } else {
        handle_out.set_offset(cur_off_out);
    }

    copied
}

pub fn sys_fallocate(fd: u64, mode: u64, offset: u64, len: u64) -> u64 {
    if mode != 0 {
        return (-(EOPNOTSUPP as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(EBADF as i64)) as u64,
    };

    let req_size = match offset.checked_add(len) {
        Some(s) => s as usize,
        None => return (-(EINVAL as i64)) as u64,
    };

    if inode.metadata().size < req_size {
        if inode.truncate(req_size).is_err() {
            return (-(super::EIO as i64)) as u64;
        }
    }

    0
}

pub fn sys_close_range(first: u64, last: u64, _flags: u64) -> u64 {
    if first > last {
        return (-(EINVAL as i64)) as u64;
    }
    let upper = last.min(1024);
    for fd in first..=upper {
        let _ = remove_file_handle(fd);
    }
    0
}

static NEXT_MEMFD_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

pub fn sys_memfd_create(name_ptr: u64, _flags: u64) -> u64 {
    let name = read_user_cstr(name_ptr, 256).unwrap_or_else(|_| alloc::string::String::from("memfd"));
    let id = NEXT_MEMFD_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let tmp_path = format!("/tmp/.memfd_{}_{}", id, name);

    let (parent_path, leaf) = match tmp_path.rfind('/') {
        Some(0) => ("/", &tmp_path[1..]),
        Some(idx) => (&tmp_path[..idx], &tmp_path[idx + 1..]),
        None => (".", tmp_path.as_str()),
    };

    let parent_node = match crate::fs::resolve_path(parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    let new_file = match parent_node.create(leaf, InodeType::File) {
        Ok(f) => f,
        Err(_) => return (-(EACCES as i64)) as u64,
    };

    // Анонимный memfd: отвязываем имя от родительского каталога
    let _ = parent_node.unlink(leaf);

    let handle = Arc::new(RegularFileHandle {
        inode: new_file,
        offset: Mutex::new(0),
        flags: core::sync::atomic::AtomicU32::new(crate::fs::O_RDWR),
        path: None,
    });

    insert_file_handle(handle)
}
