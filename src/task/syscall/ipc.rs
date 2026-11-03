use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::{
    get_file_handle, insert_file_handle, EpollInstance, PipeBuffer, PipeReadHandle,
    PipeWriteHandle,
};
use super::{read_user_cstr, validate_user_buffer, EBADF, EFAULT, EINVAL, ENOENT, EEXIST, ENODEV};

pub fn sys_pipe2(pipefd_ptr: u64, flags: u64) -> u64 {
    if !validate_user_buffer(pipefd_ptr, 8) {
        return (-(EFAULT as i64)) as u64;
    }
    let pipe = Arc::new(Mutex::new(PipeBuffer::new()));
    let read_handle = Arc::new(PipeReadHandle::new(pipe.clone(), flags as u32));
    let write_handle = Arc::new(PipeWriteHandle::new(pipe, flags as u32));

    let r_fd = insert_file_handle(read_handle);
    let w_fd = insert_file_handle(write_handle);

    unsafe {
        let ptr = pipefd_ptr as *mut i32;
        ptr.add(0).write(r_fd as i32);
        ptr.add(1).write(w_fd as i32);
    }
    0
}


pub fn sys_epoll_create1(_flags: u64) -> u64 {
    let epoll = Arc::new(EpollInstance::new());
    insert_file_handle(epoll)
}


pub fn sys_epoll_ctl(epfd: u64, op: u64, fd: u64, event_ptr: u64) -> u64 {
    let handle = match get_file_handle(epfd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.epoll_ctl(op as u32, fd, event_ptr) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_epoll_wait(epfd: u64, events_ptr: u64, maxevents: u64, timeout_ms: i64) -> u64 {
    let handle = match get_file_handle(epfd) {
        Some(h) => h,
        None => return (-(EBADF as i64)) as u64,
    };
    match handle.epoll_wait(events_ptr, maxevents as u32, timeout_ms as i32) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_eventfd2(initval: u32, flags: u32) -> u64 {
    let handle = Arc::new(crate::fs::EventFdHandle::new(initval, flags));
    insert_file_handle(handle)
}


pub fn sys_mount(source_ptr: u64, target_ptr: u64, fstype_ptr: u64, flags: u64, _data_ptr: u64) -> u64 {
    let source = match read_user_cstr(source_ptr, 256) {
        Ok(s) => s,
        Err(_) => return (-(EFAULT as i64)) as u64,
    };
    let target = match read_user_cstr(target_ptr, 256) {
        Ok(s) => s,
        Err(_) => return (-(EFAULT as i64)) as u64,
    };
    let fstype = if fstype_ptr != 0 {
        read_user_cstr(fstype_ptr, 64).unwrap_or_default()
    } else {
        alloc::string::String::new()
    };

    match crate::fs::mount(&source, &target, &fstype, flags) {
        Ok(()) => 0,
        Err(e) => match e {
            crate::fs::VfsError::NotFound => (-(ENOENT as i64)) as u64,
            crate::fs::VfsError::AlreadyExists => (-(EEXIST as i64)) as u64,
            crate::fs::VfsError::NotSupported => (-(ENODEV as i64)) as u64,
            _ => (-(EINVAL as i64)) as u64,
        },
    }
}


pub fn sys_umount2(target_ptr: u64, flags: u64) -> u64 {
    let target = match read_user_cstr(target_ptr, 256) {
        Ok(s) => s,
        Err(_) => return (-(EFAULT as i64)) as u64,
    };
    match crate::fs::umount(&target, flags as i32) {
        Ok(()) => 0,
        Err(crate::fs::VfsError::NotFound) => (-(ENOENT as i64)) as u64,
        Err(_) => (-(EINVAL as i64)) as u64,
    }
}


