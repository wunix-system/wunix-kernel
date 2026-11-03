use super::{validate_user_buffer, EFAULT, EPERM, ESRCH};

pub const RLIMIT_CPU: u64 = 0;
pub const RLIMIT_FSIZE: u64 = 1;
pub const RLIMIT_DATA: u64 = 2;
pub const RLIMIT_STACK: u64 = 3;
pub const RLIMIT_CORE: u64 = 4;
pub const RLIMIT_RSS: u64 = 5;
pub const RLIMIT_NPROC: u64 = 6;
pub const RLIMIT_NOFILE: u64 = 7;
pub const RLIMIT_MEMLOCK: u64 = 8;
pub const RLIMIT_AS: u64 = 9;

pub const RLIM_INFINITY: u64 = !0u64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RLimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

pub fn sys_getuid() -> u64 {
    crate::task::current_thread_uid() as u64
}

pub fn sys_getgid() -> u64 {
    crate::task::current_thread_gid() as u64
}

pub fn sys_geteuid() -> u64 {
    crate::task::current_thread_euid() as u64
}

pub fn sys_getegid() -> u64 {
    crate::task::current_thread_egid() as u64
}

pub fn sys_setuid(uid: u64) -> u64 {
    let new_uid = uid as u32;
    let cur_uid = crate::task::current_thread_uid();
    let cur_euid = crate::task::current_thread_euid();
    let cur_suid = crate::task::current_thread_suid();

    // Если процесс исполняется с привилегиями root (EUID == 0), вызов setuid атомарно
    // изменяет все три идентификатора (real, effective, saved).
    // Непривилегированный пользователь может сбросить euid только в свой реальный uid
    // или сохраненный suid (например, возврат прав после временного сброса).
    if cur_euid == 0 {
        crate::task::set_current_thread_uid(new_uid, new_uid, new_uid);
        0
    } else if new_uid == cur_uid || new_uid == cur_suid {
        crate::task::set_current_thread_uid(cur_uid, new_uid, cur_suid);
        0
    } else {
        (-(EPERM as i64)) as u64
    }
}

pub fn sys_setgid(gid: u64) -> u64 {
    let new_gid = gid as u32;
    let cur_gid = crate::task::current_thread_gid();
    let cur_euid = crate::task::current_thread_euid();
    let cur_sgid = crate::task::current_thread_sgid();

    if cur_euid == 0 {
        crate::task::set_current_thread_gid(new_gid, new_gid, new_gid);
        0
    } else if new_gid == cur_gid || new_gid == cur_sgid {
        crate::task::set_current_thread_gid(cur_gid, new_gid, cur_sgid);
        0
    } else {
        (-(EPERM as i64)) as u64
    }
}

pub fn sys_setsid() -> u64 {
    crate::task::current_thread_tgid()
}

pub fn sys_umask(mask: u64) -> u64 {
    crate::task::set_current_thread_umask(mask as u32) as u64
}

pub fn sys_prlimit64(
    pid: u64,
    resource: u64,
    new_limit_ptr: u64,
    old_limit_ptr: u64,
) -> u64 {
    let cur_tgid = crate::task::current_thread_tgid();
    if pid != 0 && pid != cur_tgid {
        return (-(ESRCH as i64)) as u64;
    }

    let default_limit = match resource {
        RLIMIT_STACK => RLimit {
            rlim_cur: 8 * 1024 * 1024,
            rlim_max: RLIM_INFINITY,
        },
        RLIMIT_NOFILE => RLimit {
            rlim_cur: 1024,
            rlim_max: 4096,
        },
        RLIMIT_AS | RLIMIT_DATA => RLimit {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        },
        _ => RLimit {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        },
    };

    if old_limit_ptr != 0 {
        if !validate_user_buffer(old_limit_ptr, core::mem::size_of::<RLimit>() as u64) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe {
            core::ptr::write_unaligned(old_limit_ptr as *mut RLimit, default_limit);
        }
    }

    if new_limit_ptr != 0 {
        if !validate_user_buffer(new_limit_ptr, core::mem::size_of::<RLimit>() as u64) {
            return (-(EFAULT as i64)) as u64;
        }
        let _new = unsafe {
            core::ptr::read_unaligned(new_limit_ptr as *const RLimit)
        };
    }

    0
}

pub fn sys_getgroups(size: u64, list_ptr: u64) -> u64 {
    let gid = sys_getgid() as u32;
    if size == 0 {
        return 1;
    }
    if !validate_user_buffer(list_ptr, 4) {
        return (-(EFAULT as i64)) as u64;
    }
    unsafe {
        core::ptr::write_unaligned(list_ptr as *mut u32, gid);
    }
    1
}

pub fn sys_setgroups(size: u64, list_ptr: u64) -> u64 {
    if sys_geteuid() != 0 {
        return (-(EPERM as i64)) as u64;
    }
    if size > 0 && !validate_user_buffer(list_ptr, size * 4) {
        return (-(EFAULT as i64)) as u64;
    }
    0
}

pub fn sys_setreuid(ruid: u64, euid: u64) -> u64 {
    if ruid != u64::MAX && ruid != 0xFFFF_FFFF {
        sys_setuid(ruid);
    }
    if euid != u64::MAX && euid != 0xFFFF_FFFF {
        sys_setuid(euid);
    }
    0
}

pub fn sys_setregid(rgid: u64, egid: u64) -> u64 {
    if rgid != u64::MAX && rgid != 0xFFFF_FFFF {
        sys_setgid(rgid);
    }
    if egid != u64::MAX && egid != 0xFFFF_FFFF {
        sys_setgid(egid);
    }
    0
}

pub fn sys_setresuid(ruid: u64, euid: u64, _suid: u64) -> u64 {
    sys_setreuid(ruid, euid)
}

pub fn sys_getresuid(ruid_ptr: u64, euid_ptr: u64, suid_ptr: u64) -> u64 {
    let uid = sys_getuid() as u32;
    let euid = sys_geteuid() as u32;
    if ruid_ptr != 0 {
        if !validate_user_buffer(ruid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(ruid_ptr as *mut u32, uid); }
    }
    if euid_ptr != 0 {
        if !validate_user_buffer(euid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(euid_ptr as *mut u32, euid); }
    }
    if suid_ptr != 0 {
        if !validate_user_buffer(suid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(suid_ptr as *mut u32, euid); }
    }
    0
}

pub fn sys_setresgid(rgid: u64, egid: u64, _sgid: u64) -> u64 {
    sys_setregid(rgid, egid)
}

pub fn sys_getresgid(rgid_ptr: u64, egid_ptr: u64, sgid_ptr: u64) -> u64 {
    let gid = sys_getgid() as u32;
    let egid = sys_getegid() as u32;
    if rgid_ptr != 0 {
        if !validate_user_buffer(rgid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(rgid_ptr as *mut u32, gid); }
    }
    if egid_ptr != 0 {
        if !validate_user_buffer(egid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(egid_ptr as *mut u32, egid); }
    }
    if sgid_ptr != 0 {
        if !validate_user_buffer(sgid_ptr, 4) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe { core::ptr::write_unaligned(sgid_ptr as *mut u32, egid); }
    }
    0
}

pub fn sys_capget(_hdr_ptr: u64, data_ptr: u64) -> u64 {
    if data_ptr != 0 {
        if !validate_user_buffer(data_ptr, 12) {
            return (-(EFAULT as i64)) as u64;
        }
        let caps = if sys_geteuid() == 0 { 0xFFFF_FFFFu32 } else { 0u32 };
        unsafe {
            let p = data_ptr as *mut u32;
            core::ptr::write_unaligned(p, caps);
            core::ptr::write_unaligned(p.add(1), caps);
            core::ptr::write_unaligned(p.add(2), caps);
        }
    }
    0
}

pub fn sys_capset(_hdr_ptr: u64, _data_ptr: u64) -> u64 {
    0
}
