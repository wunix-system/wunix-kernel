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

static HAS_RDRAND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

fn can_rdrand() -> bool {
    match HAS_RDRAND.load(core::sync::atomic::Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let supported = (core::arch::x86_64::__cpuid(1).ecx & (1 << 30)) != 0;
            HAS_RDRAND.store(if supported { 1 } else { 2 }, core::sync::atomic::Ordering::Relaxed);
            supported
        }
    }
}

pub fn fill_random_bytes(slice: &mut [u8]) {
    let mut seed = unsafe { core::arch::x86_64::_rdtsc() };
    let mut i = 0;
    while i < slice.len() {
        let mut val: u64 = 0;
        let mut ok = false;
        if can_rdrand() {
            let mut r_val: u64 = 0;
            let mut r_ok: u8 = 0;
            unsafe {
                core::arch::asm!(
                    "rdrand {0}",
                    "setc {1}",
                    out(reg) r_val,
                    out(reg_byte) r_ok,
                );
            }
            if r_ok != 0 {
                val = r_val;
                ok = true;
            }
        }
        if !ok {
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            let ticks = crate::arch::x86_64::timer::ticks();
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            val = tsc ^ ticks.wrapping_mul(0x9E3779B97F4A7C15) ^ seed;
            val ^= val << 13;
            val ^= val >> 7;
            val ^= val << 17;
        }
        let bytes = val.to_ne_bytes();
        let chunk = (slice.len() - i).min(8);
        slice[i..i + chunk].copy_from_slice(&bytes[..chunk]);
        i += chunk;
    }
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
