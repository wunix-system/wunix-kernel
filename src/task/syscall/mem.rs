use super::{
    validate_user_buffer, VMA_MANAGER, EFAULT, EINVAL, USER_SPACE_END,
};

pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, offset: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    match vma.mmap(addr, len, prot, flags, fd, offset) {
        Ok(vaddr) => vaddr,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_munmap(addr: u64, len: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    match vma.munmap(addr, len) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    match vma.mprotect(addr, len, prot) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_brk(new_brk: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    vma.brk(new_brk)
}

pub fn reset_process_vma(brk_start: u64) {
    let mut vma = VMA_MANAGER.lock();
    vma.reset(brk_start);
}

pub fn is_valid_user_vma_addr(addr: u64) -> bool {
    let vma = VMA_MANAGER.lock();
    vma.is_valid_addr(addr)
}


pub fn sys_arch_prctl(code: u64, addr: u64) -> u64 {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            if addr >= USER_SPACE_END {
                return (-(EFAULT as i64)) as u64;
            }
            unsafe {
                crate::arch::x86_64::syscall::write_fs_base(addr);
            }
            crate::task::set_current_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            if !validate_user_buffer(addr, 8) {
                return (-(EFAULT as i64)) as u64;
            }
            let cur_fs = unsafe { crate::arch::x86_64::syscall::read_fs_base() };
            unsafe {
                core::ptr::write_unaligned(addr as *mut u64, cur_fs);
            }
            0
        }
        _ => (-(EINVAL as i64)) as u64,
    }
}

pub fn fill_random_bytes(slice: &mut [u8]) {
    crate::arch::x86_64::cpu::get_random_bytes(slice);
}

pub fn sys_getrandom(buf_ptr: u64, len: u64, _flags: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    fill_random_bytes(slice);
    len
}
