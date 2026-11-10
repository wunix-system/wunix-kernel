use super::{validate_user_buffer, EFAULT, EINVAL, ESRCH};

pub const SCHED_OTHER: u64 = 0;
pub const SCHED_FIFO: u64 = 1;
pub const SCHED_RR: u64 = 2;
pub const SCHED_BATCH: u64 = 3;
pub const SCHED_IDLE: u64 = 5;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SchedParam {
    pub sched_priority: i32,
}

pub fn sys_sched_getscheduler(pid: u64) -> u64 {
    let cur = crate::task::current_thread_tgid();
    if pid != 0 && pid != cur {
        // Проверяем существование процесса
        if pid > 100_000 {
            return (-(ESRCH as i64)) as u64;
        }
    }
    SCHED_OTHER
}

pub fn sys_sched_setscheduler(pid: u64, policy: u64, param_ptr: u64) -> u64 {
    if !validate_user_buffer(param_ptr, core::mem::size_of::<SchedParam>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    if policy != SCHED_OTHER && policy != SCHED_BATCH && policy != SCHED_IDLE {
        return (-(EINVAL as i64)) as u64;
    }
    let cur = crate::task::current_thread_tgid();
    if pid != 0 && pid != cur && pid > 100_000 {
        return (-(ESRCH as i64)) as u64;
    }
    0
}

pub fn sys_sched_getparam(pid: u64, param_ptr: u64) -> u64 {
    if !validate_user_buffer(param_ptr, core::mem::size_of::<SchedParam>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let cur = crate::task::current_thread_tgid();
    if pid != 0 && pid != cur && pid > 100_000 {
        return (-(ESRCH as i64)) as u64;
    }
    let param = SchedParam { sched_priority: 0 };
    unsafe {
        core::ptr::write_unaligned(param_ptr as *mut SchedParam, param);
    }
    0
}

pub fn sys_sched_setparam(pid: u64, param_ptr: u64) -> u64 {
    if !validate_user_buffer(param_ptr, core::mem::size_of::<SchedParam>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let cur = crate::task::current_thread_tgid();
    if pid != 0 && pid != cur && pid > 100_000 {
        return (-(ESRCH as i64)) as u64;
    }
    0
}

pub fn sys_sched_get_priority_max(policy: u64) -> u64 {
    match policy {
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE => 0,
        SCHED_FIFO | SCHED_RR => 99,
        _ => (-(EINVAL as i64)) as u64,
    }
}

pub fn sys_sched_get_priority_min(policy: u64) -> u64 {
    match policy {
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE => 0,
        SCHED_FIFO | SCHED_RR => 1,
        _ => (-(EINVAL as i64)) as u64,
    }
}

pub fn sys_getpriority(which: u64, who: u64) -> u64 {
    if which > 2 {
        return (-(EINVAL as i64)) as u64;
    }
    let cur = crate::task::current_thread_tgid();
    if who != 0 && who != cur && who > 100_000 {
        return (-(ESRCH as i64)) as u64;
    }
    // В Linux nice варьируется от -20 до 19, системный вызов возвращает 20 - nice (диапазон 1..40)
    20
}

pub fn sys_setpriority(which: u64, who: u64, _prio: u64) -> u64 {
    if which > 2 {
        return (-(EINVAL as i64)) as u64;
    }
    let cur = crate::task::current_thread_tgid();
    if who != 0 && who != cur && who > 100_000 {
        return (-(ESRCH as i64)) as u64;
    }
    0
}

pub fn sys_getsid(pid: u64) -> u64 {
    let cur = crate::task::current_thread_tgid();
    if pid == 0 || pid == cur {
        cur
    } else if pid == 1 {
        1
    } else {
        cur
    }
}
