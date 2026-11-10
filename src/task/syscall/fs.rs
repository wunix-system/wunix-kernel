use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{
    get_file_handle, insert_file_handle, remove_file_handle, RegularFileHandle, O_CREAT,
};
use super::{
    read_user_cstr, resolve_relative_path, validate_user_buffer, EACCES, EBADF, EFAULT, EINVAL,
    EISDIR, ENOENT, ENOTDIR, ENOTTY, EIO, EPERM,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub __unused: [i64; 3],
}


pub fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    match handle.read(slice) {
        Ok(n) => n as u64,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len as usize) };
    match handle.write(slice) {
        Ok(n) => n as u64,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_open(path_ptr: u64, flags: u64, mode: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = resolve_relative_path(&path);

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
            // Проверка прав доступа к существующему файлу
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
                // Создание нового файла: проверяем права записи в родительский каталог
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

                let new_file = match parent_node.create(leaf, crate::fs::InodeType::File) {
                    Ok(f) => f,
                    Err(_) => return (-(EIO as i64)) as u64,
                };

                // Применяем umask к правам создаваемого файла
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


pub fn sys_close(fd: u64) -> u64 {
    if remove_file_handle(fd).is_some() {
        0
    } else {
        (-(EBADF as i64)) as u64
    }
}


pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };

    match cmd {
        0 | 1030 => {
            // F_DUPFD (0) / F_DUPFD_CLOEXEC (1030)
            insert_file_handle(handle)
        }
        1 => 0, // F_GETFD (FD_CLOEXEC = 0)
        2 => 0, // F_SETFD
        3 => handle.get_flags() as u64, // F_GETFL
        4 => {
            // F_SETFL (установка O_NONBLOCK)
            handle.set_flags(arg as u32);
            0
        }
        _ => 0,
    }
}


pub fn sys_fstat(fd: u64, statbuf_ptr: u64) -> u64 {
    if !validate_user_buffer(statbuf_ptr, core::mem::size_of::<Stat>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };

    let mut stat = Stat::default();
    stat.st_nlink = 1;
    stat.st_blksize = 4096;

    if handle.is_tty() {
        stat.st_mode = 0o020666; // S_IFCHR | 0666
        stat.st_rdev = 0x0500;   // major 5, minor 0 (/dev/tty)
    } else if let Some(inode) = handle.as_inode() {
        let meta = inode.metadata();
        stat.st_size = meta.size as i64;
        stat.st_ino = meta.inode_id;
        let type_bits = match meta.itype {
            crate::fs::InodeType::Directory => 0o040000,
            crate::fs::InodeType::File => 0o100000,
            crate::fs::InodeType::Symlink => 0o120000,
            crate::fs::InodeType::BlockDevice => 0o060000,
            crate::fs::InodeType::CharDevice => 0o020000,
        };
        stat.st_mode = type_bits | (meta.permissions as u32 & 0o7777);
        stat.st_uid = meta.uid;
        stat.st_gid = meta.gid;
    } else {
        stat.st_mode = 0o010600; // S_IFIFO
    }

    unsafe {
        core::ptr::write_unaligned(statbuf_ptr as *mut Stat, stat);
    }
    0
}


pub fn sys_lseek(fd: u64, offset: i64, whence: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let file_size = handle.as_inode().map(|i| i.metadata().size).unwrap_or(0);
    let cur_off = handle.offset();
    let new_off = match whence {
        0 => offset,                         // SEEK_SET
        1 => cur_off as i64 + offset,         // SEEK_CUR
        2 => file_size as i64 + offset,       // SEEK_END
        _ => return (-(EINVAL as i64)) as u64,
    };
    if new_off < 0 {
        return (-(EINVAL as i64)) as u64;
    }
    handle.set_offset(new_off as usize);
    new_off as u64
}


pub const TCGETS: u64 = 0x5401;
pub const TCSETS: u64 = 0x5402;
pub const TCSETSW: u64 = 0x5403;
pub const TCSETSF: u64 = 0x5404;
pub const TIOCSCTTY: u64 = 0x540E;
pub const TIOCGPGRP: u64 = 0x540F;
pub const TIOCSPGRP: u64 = 0x5410;
pub const TIOCGWINSZ: u64 = 0x5413;
pub const TIOCSWINSZ: u64 = 0x5414;
pub const FIONREAD: u64 = 0x541B;
pub const FIONBIO: u64 = 0x5421;

/// Структура параметров терминала согласно стандарту Linux x86_64 ABI (размер ровно 60 байт).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

static CURRENT_TERMIOS: Mutex<Termios> = Mutex::new(Termios {
    c_iflag: 0x0500, // ICRNL | IXON
    c_oflag: 0x0005, // OPOST | ONLCR
    c_cflag: 0x00bf, // CS8 | CREAD | B38400
    c_lflag: 0x8a3b, // ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN
    c_line: 0,
    c_cc: [
        3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    c_ispeed: 38400,
    c_ospeed: 38400,
});

/// Проверяет, включена ли генерация сигналов (ISIG = bit 0 в c_lflag).
/// При переходе полноэкранных редакторов (nano, vi) в небуферизованный raw mode этот флаг сбрасывается,
/// и комбинации вроде Ctrl+C не должны аварийно прерывать процесс, а передаваться как обычный управляющий байт 0x03.
pub fn is_isig_enabled() -> bool {
    let tio = CURRENT_TERMIOS.lock();
    (tio.c_lflag & 0x0001) != 0
}

/// Проверяет, включена ли трансляция CR в NL на вводе (ICRNL = 0x0100 в c_iflag).
/// В каноническом режиме шелла нажатие клавиши Enter (CR \r, 0x0D) транслируется в LF (\n, 0x0A).
/// При переходе полноэкранных редакторов (nano) в raw-режим этот флаг сбрасывается,
/// и nano ожидает оригинальный байт \r для добавления новой строки в буфер.
pub fn is_icrnl_enabled() -> bool {
    let tio = CURRENT_TERMIOS.lock();
    (tio.c_iflag & 0x0100) != 0
}

pub fn sys_ioctl(fd: u64, cmd: u64, arg: u64) -> u64 {
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    struct WinSize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    let is_tty_fd = fd <= 2 || get_file_handle(fd).map_or(false, |h| h.is_tty());
    if is_tty_fd {
        match cmd {
            TIOCGWINSZ => {
                if !validate_user_buffer(arg, 8) {
                    return (-(EFAULT as i64)) as u64;
                }
                let (rows, cols) = if let Some(console) = crate::drivers::display::CONSOLE.lock().as_ref() {
                    (console.rows as u16, console.cols as u16)
                } else {
                    (25, 80)
                };
                let ws = WinSize {
                    ws_row: rows,
                    ws_col: cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                unsafe {
                    core::ptr::write_unaligned(arg as *mut WinSize, ws);
                }
                return 0;
            }
            TIOCSWINSZ => return 0,
            TCGETS => {
                let termios_size = core::mem::size_of::<Termios>() as u64;
                if !validate_user_buffer(arg, termios_size) {
                    return (-(EFAULT as i64)) as u64;
                }
                let tio = *CURRENT_TERMIOS.lock();
                unsafe {
                    core::ptr::write_unaligned(arg as *mut Termios, tio);
                }
                return 0;
            }
            TCSETS | TCSETSW | TCSETSF => {
                let termios_size = core::mem::size_of::<Termios>() as u64;
                if !validate_user_buffer(arg, termios_size) {
                    return (-(EFAULT as i64)) as u64;
                }
                let new_tio = unsafe { core::ptr::read_unaligned(arg as *const Termios) };
                *CURRENT_TERMIOS.lock() = new_tio;
                return 0;
            }
            TIOCGPGRP => {
                if !validate_user_buffer(arg, 4) {
                    return (-(EFAULT as i64)) as u64;
                }
                let pgrp = crate::task::syscall::proc::sys_getpgrp() as u32;
                unsafe {
                    core::ptr::write_unaligned(arg as *mut u32, pgrp);
                }
                return 0;
            }
            TIOCSPGRP | TIOCSCTTY => return 0,
            FIONREAD => {
                if !validate_user_buffer(arg, 4) {
                    return (-(EFAULT as i64)) as u64;
                }
                let count = (crate::fs::handle::stdin_buffer_len() + crate::drivers::keyboard::key_queue_len()) as i32;
                unsafe {
                    core::ptr::write_unaligned(arg as *mut i32, count);
                }
                return 0;
            }
            _ => {}
        }
    }

    match cmd {
        FIONBIO => {
            if !validate_user_buffer(arg, 4) {
                return (-(EFAULT as i64)) as u64;
            }
            let val = unsafe { core::ptr::read_unaligned(arg as *const i32) };
            if let Some(handle) = get_file_handle(fd) {
                if val != 0 {
                    handle.set_flags(handle.get_flags() | crate::fs::O_NONBLOCK);
                } else {
                    handle.set_flags(handle.get_flags() & !crate::fs::O_NONBLOCK);
                }
                0
            } else {
                (-(EBADF as i64)) as u64
            }
        }
        FIONREAD => {
            if !validate_user_buffer(arg, 4) {
                return (-(EFAULT as i64)) as u64;
            }
            if get_file_handle(fd).is_some() {
                unsafe { core::ptr::write_unaligned(arg as *mut i32, 0); }
                0
            } else {
                (-(EBADF as i64)) as u64
            }
        }
        _ => (-(ENOTTY as i64)) as u64,
    }
}


pub fn sys_unlink(path_ptr: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = resolve_relative_path(&path);

    let (parent_path, leaf) = match target_path.rfind('/') {
        Some(0) => ("/", &target_path[1..]),
        Some(idx) => (&target_path[..idx], &target_path[idx + 1..]),
        None => (".", target_path.as_str()),
    };

    let parent_node = match crate::fs::resolve_path(parent_path, true) {
        Ok(p) => p,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    // Удаление файла модифицирует родительский каталог — требуются права записи и исполнения
    if let Err(_) = crate::fs::check_permission(&parent_node, false, true, true) {
        return (-(EACCES as i64)) as u64;
    }

    let file_node = match parent_node.lookup(leaf) {
        Ok(f) => f,
        Err(_) => return (-(ENOENT as i64)) as u64,
    };

    // Sticky bit (0o1000): в общих каталогах (например, /tmp с режимом 1777)
    // удаление разрешено исключительно владельцу файла, владельцу каталога или root
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
        Err(crate::fs::VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(crate::fs::VfsError::IsADirectory) => (-(EISDIR as i64)) as u64,
        Err(crate::fs::VfsError::PermissionDenied) => (-(EACCES as i64)) as u64,
        Err(_) => (-(EIO as i64)) as u64,
    }
}


pub fn sys_getdents64(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }

    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };

    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(EBADF as i64)) as u64,
    };

    let entries = match inode.readdir() {
        Ok(e) => e,
        Err(crate::fs::VfsError::NotADirectory) => return (-(ENOTDIR as i64)) as u64,
        Err(_) => return (-(EBADF as i64)) as u64,
    };

    let current_offset = handle.offset();
    if current_offset >= entries.len() {
        return 0;
    }

    let mut bytes_written = 0usize;
    let dst = buf_ptr as *mut u8;
    let mut entries_read = 0;

    for (idx, entry) in entries[current_offset..].iter().enumerate() {
        let entry_idx = current_offset + idx;
        let name_bytes = entry.name.as_bytes();
        let rec_len = ((19 + name_bytes.len() + 1) + 7) & !7;

        if bytes_written + rec_len > len as usize {
            break;
        }

        unsafe {
            let rec_ptr = dst.add(bytes_written);
            core::ptr::write_unaligned(rec_ptr as *mut u64, (entry_idx + 1) as u64);
            core::ptr::write_unaligned(rec_ptr.add(8) as *mut i64, (entry_idx + 1) as i64);
            core::ptr::write_unaligned(rec_ptr.add(16) as *mut u16, rec_len as u16);

            let dtype: u8 = match entry.itype {
                crate::fs::InodeType::Directory => 4,
                crate::fs::InodeType::File => 8,
                crate::fs::InodeType::Symlink => 10,
                _ => 0,
            };
            *rec_ptr.add(18) = dtype;

            core::ptr::copy_nonoverlapping(name_bytes.as_ptr(), rec_ptr.add(19), name_bytes.len());
            *rec_ptr.add(19 + name_bytes.len()) = 0;
        }

        bytes_written += rec_len;
        entries_read += 1;
    }

    handle.set_offset(current_offset + entries_read);
    bytes_written as u64
}

pub fn sys_ftruncate(fd: u64, length: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(EINVAL as i64)) as u64,
    };
    match inode.truncate(length as usize) {
        Ok(()) => 0,
        Err(_) => (-(EIO as i64)) as u64,
    }
}

pub fn sys_pread64(fd: u64, buf_ptr: u64, len: u64, offset: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(29i64)) as u64, // -ESPIPE: каналы и сокеты не поддерживают позиционирование
    };
    let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    match inode.read_at(offset as usize, slice) {
        Ok(n) => n as u64,
        Err(_) => (-(EIO as i64)) as u64,
    }
}

pub fn sys_pwrite64(fd: u64, buf_ptr: u64, len: u64, offset: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(29i64)) as u64, // -ESPIPE
    };
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len as usize) };
    match inode.write_at(offset as usize, slice) {
        Ok(n) => n as u64,
        Err(_) => (-(EIO as i64)) as u64,
    }
}

pub fn sys_dup(fd: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    insert_file_handle(handle)
}

pub fn sys_dup3(oldfd: u64, newfd: u64, _flags: u64) -> u64 {
    if oldfd == newfd {
        return (-(EINVAL as i64)) as u64;
    }
    let handle = match get_file_handle(oldfd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let _ = remove_file_handle(newfd);
    crate::fs::insert_file_handle_at(newfd, handle);
    newfd
}

pub fn sys_chmod(path_ptr: u64, mode: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&path);
    let node = match crate::fs::resolve_path(&norm, true) {
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

pub fn sys_fchmod(fd: u64, mode: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(EBADF as i64)) as u64,
    };

    let meta = inode.metadata();
    let euid = crate::task::current_thread_euid();
    if euid != 0 && euid != meta.uid {
        return (-(EPERM as i64)) as u64;
    }

    match inode.chmod((mode & 0o7777) as u16) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_chown(path_ptr: u64, uid: u64, gid: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return (-(e as i64)) as u64,
    };
    let norm = resolve_relative_path(&path);
    let node = match crate::fs::resolve_path(&norm, true) {
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

pub fn sys_fchown(fd: u64, uid: u64, gid: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let inode = match handle.as_inode() {
        Some(i) => i,
        None => return (-(EBADF as i64)) as u64,
    };

    let euid = crate::task::current_thread_euid();
    if euid != 0 {
        return (-(EPERM as i64)) as u64;
    }

    match inode.chown(uid as u32, gid as u32) {
        Ok(()) => 0,
        Err(_) => (-(EACCES as i64)) as u64,
    }
}

pub fn sys_sync() -> u64 {
    // В Wunix файловые операции RAMFS атомарны, а изменения на дисках FAT32 сбрасываются.
    0
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IoVec {
    pub iov_base: u64,
    pub iov_len: u64,
}

pub fn sys_readv(fd: u64, iov_ptr: u64, iovcnt: u64) -> u64 {
    if iovcnt == 0 || iovcnt > 1024 {
        return (-(EINVAL as i64)) as u64;
    }
    let total_bytes = match (iovcnt as usize).checked_mul(core::mem::size_of::<IoVec>()) {
        Some(b) => b as u64,
        None => return (-(EINVAL as i64)) as u64,
    };
    if !validate_user_buffer(iov_ptr, total_bytes) {
        return (-(EFAULT as i64)) as u64;
    }

    let mut total_read = 0u64;
    for i in 0..iovcnt {
        let entry_ptr = (iov_ptr + i * core::mem::size_of::<IoVec>() as u64) as *const IoVec;
        let iov = unsafe { core::ptr::read_unaligned(entry_ptr) };
        if iov.iov_len == 0 {
            continue;
        }
        if !validate_user_buffer(iov.iov_base, iov.iov_len) {
            if total_read > 0 {
                return total_read;
            }
            return (-(EFAULT as i64)) as u64;
        }
        let res = sys_read(fd, iov.iov_base, iov.iov_len);
        if (res as i64) < 0 {
            if total_read > 0 {
                return total_read;
            }
            return res;
        }
        total_read += res;
        if res < iov.iov_len {
            break;
        }
    }
    total_read
}

pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> u64 {
    if iovcnt == 0 || iovcnt > 1024 {
        return (-(EINVAL as i64)) as u64;
    }
    let total_bytes = match (iovcnt as usize).checked_mul(core::mem::size_of::<IoVec>()) {
        Some(b) => b as u64,
        None => return (-(EINVAL as i64)) as u64,
    };
    if !validate_user_buffer(iov_ptr, total_bytes) {
        return (-(EFAULT as i64)) as u64;
    }

    let mut total_written = 0u64;
    for i in 0..iovcnt {
        let entry_ptr = (iov_ptr + i * core::mem::size_of::<IoVec>() as u64) as *const IoVec;
        let iov = unsafe { core::ptr::read_unaligned(entry_ptr) };
        if iov.iov_len == 0 {
            continue;
        }
        if !validate_user_buffer(iov.iov_base, iov.iov_len) {
            if total_written > 0 {
                return total_written;
            }
            return (-(EFAULT as i64)) as u64;
        }
        let res = sys_write(fd, iov.iov_base, iov.iov_len);
        if (res as i64) < 0 {
            if total_written > 0 {
                return total_written;
            }
            return res;
        }
        total_written += res;
        if res < iov.iov_len {
            break;
        }
    }
    total_written
}

pub fn sys_fsync(fd: u64) -> u64 {
    if fd <= 2 {
        return 0;
    }
    if let Some(_handle) = get_file_handle(fd) {
        crate::task::syscall::fs::sys_sync()
    } else {
        (-(EBADF as i64)) as u64
    }
}

pub fn sys_fdatasync(fd: u64) -> u64 {
    sys_fsync(fd)
}

pub fn sys_flock(fd: u64, _operation: u64) -> u64 {
    if get_file_handle(fd).is_some() || fd <= 2 {
        0
    } else {
        (-(EBADF as i64)) as u64
    }
}

pub fn sys_fchdir(fd: u64) -> u64 {
    if let Some(handle) = get_file_handle(fd) {
        if let Some(inode) = handle.as_inode() {
            if inode.metadata().itype != crate::fs::InodeType::Directory {
                return (-(ENOTDIR as i64)) as u64;
            }
            if let Some(path) = handle.path() {
                super::set_cwd(&path);
            }
            0
        } else {
            (-(ENOTDIR as i64)) as u64
        }
    } else {
        (-(EBADF as i64)) as u64
    }
}

pub fn sys_utimensat(dirfd: i64, path_ptr: u64, _times_ptr: u64, _flags: u64) -> u64 {
    if path_ptr == 0 {
        if dirfd >= 0 && get_file_handle(dirfd as u64).is_some() {
            return 0;
        }
        return (-(EBADF as i64)) as u64;
    }
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };
    let target_path = resolve_relative_path(&path);
    if crate::fs::resolve_path(&target_path, true).is_ok() {
        0
    } else {
        (-(ENOENT as i64)) as u64
    }
}

pub fn sys_utimes(path_ptr: u64, times_ptr: u64) -> u64 {
    sys_utimensat(-100, path_ptr, times_ptr, 0)
}




