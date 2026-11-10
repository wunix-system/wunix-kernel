use core::sync::atomic::{AtomicU64, Ordering};
use super::{EINTR, EINVAL, EPERM, ESRCH};

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
        match sig {
            SIGINT => {
                if crate::task::syscall::proc::is_current_interactive_shell() {
                    crate::fs::handle::flush_stdin();
                    crate::drivers::keyboard::clear_key_queue();
                    crate::serial_println!("^C");
                    crate::println!("^C");
                    0
                } else {
                    super::proc::sys_exit_group(128 + sig)
                }
            }
            SIGKILL | SIGTERM | SIGQUIT | SIGHUP | SIGPIPE => {
                super::proc::sys_exit_group(128 + sig)
            }
            _ => 0,
        }
    } else {
        match sig {
            SIGKILL | SIGTERM | SIGQUIT | SIGHUP | SIGPIPE | SIGINT => {
                crate::task::kill_process_threads(target_tgid);
                crate::fs::remove_process_fd_table(target_tgid);
                super::remove_process_cwd(target_tgid);
                super::proc::record_process_exit(target_tgid, (sig as i32 & 0x7f) | 0x80);
                0
            }
            _ => 0,
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
