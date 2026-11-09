use crate::fs::get_file_handle;
use super::{validate_user_buffer, EBADF, EFAULT};

pub fn sys_connect(fd: u64, addr_ptr: u64, _addr_len: u64) -> u64 {
    if !validate_user_buffer(addr_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let addr = unsafe { core::ptr::read_unaligned(addr_ptr as *const crate::fs::SockAddrIn) };
    match handle.socket_connect(&addr) {
        Ok(()) => 0,
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_bind(fd: u64, addr_ptr: u64, _addr_len: u64) -> u64 {
    if !validate_user_buffer(addr_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let addr = unsafe { core::ptr::read_unaligned(addr_ptr as *const crate::fs::SockAddrIn) };
    match handle.socket_bind(&addr) {
        Ok(()) => 0,
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_listen(fd: u64, backlog: i32) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.socket_listen(backlog) {
        Ok(()) => 0,
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_accept4(fd: u64, addr_ptr: u64, addrlen_ptr: u64, flags: u32) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.socket_accept(flags) {
        Ok((new_fd, peer_addr)) => {
            if addr_ptr != 0 && validate_user_buffer(addr_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
                unsafe {
                    core::ptr::write_unaligned(addr_ptr as *mut crate::fs::SockAddrIn, peer_addr);
                }
            }
            if addrlen_ptr != 0 && validate_user_buffer(addrlen_ptr, 4) {
                unsafe {
                    core::ptr::write_unaligned(addrlen_ptr as *mut u32, core::mem::size_of::<crate::fs::SockAddrIn>() as u32);
                }
            }
            new_fd
        }
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_sendto(fd: u64, buf_ptr: u64, len: u64, flags: i32, dest_ptr: u64, _dest_len: u64) -> u64 {
    if len > 0 && !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let slice = if len > 0 {
        unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len as usize) }
    } else {
        &[]
    };
    let dest_addr = if dest_ptr != 0 && validate_user_buffer(dest_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
        Some(unsafe { core::ptr::read_unaligned(dest_ptr as *const crate::fs::SockAddrIn) })
    } else {
        None
    };
    match handle.socket_sendto(slice, flags, dest_addr.as_ref()) {
        Ok(n) => n as u64,
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_recvfrom(fd: u64, buf_ptr: u64, len: u64, flags: i32, src_ptr: u64, src_len_ptr: u64) -> u64 {
    if len > 0 && !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    let slice = if len > 0 {
        unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) }
    } else {
        &mut []
    };
    match handle.socket_recvfrom(slice, flags) {
        Ok((n, src_addr)) => {
            if src_ptr != 0 && validate_user_buffer(src_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
                unsafe {
                    core::ptr::write_unaligned(src_ptr as *mut crate::fs::SockAddrIn, src_addr);
                }
            }
            if src_len_ptr != 0 && validate_user_buffer(src_len_ptr, 4) {
                unsafe {
                    core::ptr::write_unaligned(src_len_ptr as *mut u32, core::mem::size_of::<crate::fs::SockAddrIn>() as u32);
                }
            }
            n as u64
        }
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_getsockname(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.socket_getsockname() {
        Ok(addr) => {
            if addr_ptr != 0 && validate_user_buffer(addr_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
                unsafe {
                    core::ptr::write_unaligned(addr_ptr as *mut crate::fs::SockAddrIn, addr);
                }
            }
            if addrlen_ptr != 0 && validate_user_buffer(addrlen_ptr, 4) {
                unsafe {
                    core::ptr::write_unaligned(addrlen_ptr as *mut u32, core::mem::size_of::<crate::fs::SockAddrIn>() as u32);
                }
            }
            0
        }
        Err(e) => (-(e.abs())) as u64,
    }
}


pub fn sys_getpeername(fd: u64, addr_ptr: u64, addrlen_ptr: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.socket_getpeername() {
        Ok(addr) => {
            if addr_ptr != 0 && validate_user_buffer(addr_ptr, core::mem::size_of::<crate::fs::SockAddrIn>() as u64) {
                unsafe {
                    core::ptr::write_unaligned(addr_ptr as *mut crate::fs::SockAddrIn, addr);
                }
            }
            if addrlen_ptr != 0 && validate_user_buffer(addrlen_ptr, 4) {
                unsafe {
                    core::ptr::write_unaligned(addrlen_ptr as *mut u32, core::mem::size_of::<crate::fs::SockAddrIn>() as u32);
                }
            }
            0
        }
        Err(e) => (-(e.abs())) as u64,
    }
}

pub fn sys_shutdown(fd: u64, how: u64) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.socket_shutdown(how as i32) {
        Ok(()) => 0,
        Err(e) => (-(e.abs())) as u64,
    }
}

pub fn sys_setsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval_ptr: u64,
    optlen: u64,
) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    if optlen > 0 && !validate_user_buffer(optval_ptr, optlen) {
        return (-(EFAULT as i64)) as u64;
    }
    let slice = if optlen > 0 {
        unsafe { core::slice::from_raw_parts(optval_ptr as *const u8, optlen as usize) }
    } else {
        &[]
    };
    match handle.socket_setsockopt(level as i32, optname as i32, slice) {
        Ok(()) => 0,
        Err(e) => (-(e.abs())) as u64,
    }
}

pub fn sys_getsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval_ptr: u64,
    optlen_ptr: u64,
) -> u64 {
    let handle = match get_file_handle(fd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    if !validate_user_buffer(optlen_ptr, 4) {
        return (-(EFAULT as i64)) as u64;
    }
    let max_len = unsafe { *(optlen_ptr as *const u32) } as usize;
    match handle.socket_getsockopt(level as i32, optname as i32) {
        Ok(bytes) => {
            let to_copy = bytes.len().min(max_len);
            if to_copy > 0 {
                if !validate_user_buffer(optval_ptr, to_copy as u64) {
                    return (-(EFAULT as i64)) as u64;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), optval_ptr as *mut u8, to_copy);
                    *(optlen_ptr as *mut u32) = to_copy as u32;
                }
            } else {
                unsafe {
                    *(optlen_ptr as *mut u32) = 0;
                }
            }
            0
        }
        Err(e) => (-(e.abs())) as u64,
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Msghdr {
    pub msg_name: u64,
    pub msg_namelen: u32,
    pub msg_pad1: u32,
    pub msg_iov: u64,
    pub msg_iovlen: u64,
    pub msg_control: u64,
    pub msg_controllen: u64,
    pub msg_flags: i32,
    pub msg_pad2: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Iovec {
    pub iov_base: u64,
    pub iov_len: u64,
}

pub fn sys_sendmsg(fd: u64, msg_ptr: u64, flags: u64) -> u64 {
    if !validate_user_buffer(msg_ptr, core::mem::size_of::<Msghdr>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let hdr = unsafe { *(msg_ptr as *const Msghdr) };
    if hdr.msg_iovlen == 0 || hdr.msg_iov == 0 {
        return 0;
    }
    if !validate_user_buffer(hdr.msg_iov, core::mem::size_of::<Iovec>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let iov = unsafe { *(hdr.msg_iov as *const Iovec) };
    let dest_ptr = hdr.msg_name;
    let dest_len = hdr.msg_namelen as u64;
    super::sys_sendto(fd, iov.iov_base, iov.iov_len, flags as i32, dest_ptr, dest_len)
}

pub fn sys_recvmsg(fd: u64, msg_ptr: u64, flags: u64) -> u64 {
    if !validate_user_buffer(msg_ptr, core::mem::size_of::<Msghdr>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let hdr = unsafe { *(msg_ptr as *const Msghdr) };
    if hdr.msg_iovlen == 0 || hdr.msg_iov == 0 {
        return 0;
    }
    if !validate_user_buffer(hdr.msg_iov, core::mem::size_of::<Iovec>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let iov = unsafe { *(hdr.msg_iov as *const Iovec) };
    let src_ptr = hdr.msg_name;
    let srclen_ptr = if hdr.msg_namelen > 0 {
        msg_ptr + 8 // смещение поля msg_namelen в struct msghdr
    } else {
        0
    };
    super::sys_recvfrom(fd, iov.iov_base, iov.iov_len, flags as i32, src_ptr, srclen_ptr)
}

pub fn sys_socketpair(domain: u64, sock_type: u64, _protocol: u64, sv_ptr: u64) -> u64 {
    if !validate_user_buffer(sv_ptr, 8) {
        return (-(EFAULT as i64)) as u64;
    }
    // Поддерживаем AF_UNIX (1) / AF_LOCAL (1)
    if domain != 1 {
        return (-(super::EAFNOSUPPORT as i64)) as u64;
    }

    let flags = (sock_type & 0o4000) as u32; // O_NONBLOCK
    let (sock_a, sock_b) = crate::fs::create_socketpair(flags);
    let fd_a = crate::fs::insert_file_handle(sock_a);
    let fd_b = crate::fs::insert_file_handle(sock_b);

    unsafe {
        let ptr = sv_ptr as *mut i32;
        ptr.add(0).write(fd_a as i32);
        ptr.add(1).write(fd_b as i32);
    }
    0
}

