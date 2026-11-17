use core::sync::atomic::{AtomicU64, Ordering};
use super::{validate_user_buffer, EFAULT, EINTR, EINVAL, EPERM, ESRCH, Timeval};

pub const SIGHUP: u64 = 1;
pub const SIGINT: u64 = 2;
pub const SIGQUIT: u64 = 3;
pub const SIGKILL: u64 = 9;
pub const SIGUSR1: u64 = 10;
pub const SIGUSR2: u64 = 12;
pub const SIGPIPE: u64 = 13;
pub const SIGALRM: u64 = 14;
pub const SIGTERM: u64 = 15;
pub const SIGCHLD: u64 = 17;
pub const SIGCONT: u64 = 18;
pub const SIGSTOP: u64 = 19;
pub const SIGTSTP: u64 = 20;
pub const SIGWINCH: u64 = 28;

static ALARM_DEADLINE: AtomicU64 = AtomicU64::new(0);

pub fn sys_kill(pid: i64, sig: u64) -> u64 {
    if sig > 64 {
        return (-(EINVAL as i64)) as u64;
    }

    let cur_tgid = crate::task::current_thread_tgid();
    let cur_euid = crate::task::current_thread_euid();

    let target_tgid = if pid > 0 {
        pid as u64
    } else if pid == 0 {
        cur_tgid
    } else if pid == -1 {
        // Broadcast всем процессам кроме init (1) и себя
        if sig == 0 {
            return 0;
        }
        return 0;
    } else {
        // Группа процессов (-pid)
        (-pid) as u64
    };

    if !crate::task::process_exists(target_tgid) {
        return (-(ESRCH as i64)) as u64;
    }

    // Проверка прав: суперпользователь root может слать сигналы любому процессу
    if cur_euid != 0 && target_tgid != cur_tgid {
        return (-(EPERM as i64)) as u64;
    }

    // Сигнал 0 используется стандартом POSIX для проверки существования и прав
    if sig == 0 {
        return 0;
    }

    if target_tgid == cur_tgid {
        crate::task::signal::raise_signal(cur_tgid, sig);
        0
    } else {
        match sig {
            SIGKILL => {
                crate::task::kill_process_threads(target_tgid);
                crate::fs::remove_process_fd_table(target_tgid);
                super::remove_process_cwd(target_tgid);
                crate::task::signal::cleanup_process_signals(target_tgid);
                super::proc::record_process_exit(target_tgid, (sig as i32 & 0x7f) | 0x80);
                0
            }
            _ => {
                crate::task::signal::raise_signal(target_tgid, sig);
                0
            }
        }
    }
}

pub fn sys_tgkill(tgid: i64, _tid: i64, sig: u64) -> u64 {
    sys_kill(tgid, sig)
}

pub fn sys_tkill(tid: i64, sig: u64) -> u64 {
    sys_kill(tid, sig)
}

pub fn sys_pause() -> u64 {
    loop {
        crate::task::block_current(None);
        if crate::task::signal::is_sigint_pending_for_current() {
            crate::task::signal::clear_sigint();
            return (-(EINTR as i64)) as u64;
        }
    }
}

pub fn sys_alarm(seconds: u64) -> u64 {
    let now = crate::arch::x86_64::timer::ticks();
    let old_dl = ALARM_DEADLINE.load(Ordering::Relaxed);
    let rem = if old_dl > now {
        (old_dl - now) / 100
    } else {
        0
    };

    if seconds == 0 {
        ALARM_DEADLINE.store(0, Ordering::Relaxed);
    } else {
        let new_dl = now.saturating_add(seconds.saturating_mul(100));
        ALARM_DEADLINE.store(new_dl, Ordering::Relaxed);
    }

    rem
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Itimerval {
    pub it_interval: Timeval,
    pub it_value: Timeval,
}

pub fn sys_getitimer(which: u64, curr_value_ptr: u64) -> u64 {
    if which > 2 {
        return (-(EINVAL as i64)) as u64;
    }
    if curr_value_ptr != 0 {
        if !validate_user_buffer(curr_value_ptr, core::mem::size_of::<Itimerval>() as u64) {
            return (-(EFAULT as i64)) as u64;
        }
        let now = crate::arch::x86_64::timer::ticks();
        let dl = ALARM_DEADLINE.load(Ordering::Relaxed);
        let (sec, usec) = if which == 0 && dl > now {
            let rem_ticks = dl - now;
            let sec = (rem_ticks / 100) as i64;
            let usec = ((rem_ticks % 100) * 10_000) as i64;
            (sec, usec)
        } else {
            (0, 0)
        };
        let it = Itimerval {
            it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
            it_value: Timeval { tv_sec: sec, tv_usec: usec },
        };
        unsafe {
            core::ptr::write_unaligned(curr_value_ptr as *mut Itimerval, it);
        }
    }
    0
}

pub fn sys_setitimer(which: u64, new_value_ptr: u64, old_value_ptr: u64) -> u64 {
    if which > 2 {
        return (-(EINVAL as i64)) as u64;
    }
    if old_value_ptr != 0 {
        let ret = sys_getitimer(which, old_value_ptr);
        if ret != 0 {
            return ret;
        }
    }
    if new_value_ptr != 0 {
        if !validate_user_buffer(new_value_ptr, core::mem::size_of::<Itimerval>() as u64) {
            return (-(EFAULT as i64)) as u64;
        }
        let new_it: Itimerval = unsafe { core::ptr::read_unaligned(new_value_ptr as *const Itimerval) };
        if which == 0 {
            let now = crate::arch::x86_64::timer::ticks();
            if new_it.it_value.tv_sec == 0 && new_it.it_value.tv_usec == 0 {
                ALARM_DEADLINE.store(0, Ordering::Relaxed);
            } else {
                let sec_ticks = (new_it.it_value.tv_sec.max(0) as u64).saturating_mul(100);
                let usec_ticks = (new_it.it_value.tv_usec.max(0) as u64) / 10_000;
                let delta = sec_ticks.saturating_add(usec_ticks).max(1);
                let new_dl = now.saturating_add(delta);
                ALARM_DEADLINE.store(new_dl, Ordering::Relaxed);
            }
        }
    }
    0
}
