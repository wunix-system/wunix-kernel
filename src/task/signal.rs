use alloc::collections::BTreeMap;
use spin::Mutex;

use super::scheduler;
use super::thread::ThreadId;

pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_SIGALTSTACK: u64 = 131;

pub const NSIG: usize = 65;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SigAction {
    pub sa_handler: u64,
    pub sa_flags: u64,
    pub sa_restorer: u64,
    pub sa_mask: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct StackT {
    pub ss_sp: u64,
    pub ss_flags: i32,
    pub _pad: u32,
    pub ss_size: u64,
}

static SIGNAL_ACTIONS: Mutex<[SigAction; NSIG]> =
    Mutex::new([SigAction { sa_handler: 0, sa_flags: 0, sa_restorer: 0, sa_mask: 0 }; NSIG]);

static THREAD_SIGMASKS: Mutex<BTreeMap<ThreadId, u64>> = Mutex::new(BTreeMap::new());
static THREAD_ALTSTACKS: Mutex<BTreeMap<ThreadId, StackT>> = Mutex::new(BTreeMap::new());

static SIGINT_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Вызывается при нажатии комбинации Ctrl+C с клавиатуры или поступлении ASCII 3 из COM1
pub fn raise_sigint() {
    SIGINT_PENDING.store(true, core::sync::atomic::Ordering::SeqCst);
}

pub fn is_sigint_pending() -> bool {
    SIGINT_PENDING.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn clear_sigint() {
    SIGINT_PENDING.store(false, core::sync::atomic::Ordering::SeqCst);
}

pub fn sys_rt_sigaction(signum: u64, act_ptr: u64, oldact_ptr: u64, _sigsetsize: u64) -> u64 {
    if signum == 0 || signum >= (NSIG as u64) {
        return (-(22i64)) as u64; // -EINVAL
    }

    let mut actions = SIGNAL_ACTIONS.lock();

    if oldact_ptr != 0 {
        if oldact_ptr >= USER_SPACE_END || oldact_ptr + core::mem::size_of::<SigAction>() as u64 > USER_SPACE_END {
            return (-(14i64)) as u64; // -EFAULT
        }
        unsafe {
            core::ptr::write_unaligned(oldact_ptr as *mut SigAction, actions[signum as usize]);
        }
    }

    if act_ptr != 0 {
        if act_ptr >= USER_SPACE_END || act_ptr + core::mem::size_of::<SigAction>() as u64 > USER_SPACE_END {
            return (-(14i64)) as u64;
        }
        // SIGKILL (9) и SIGSTOP (19) нельзя перехватить или игнорировать по спецификации POSIX
        if signum == 9 || signum == 19 {
            return (-(22i64)) as u64;
        }
        let new_act = unsafe { core::ptr::read_unaligned(act_ptr as *const SigAction) };
        actions[signum as usize] = new_act;
    }

    0
}

pub fn sys_rt_sigprocmask(how: i32, set_ptr: u64, oldset_ptr: u64, _sigsetsize: u64) -> u64 {
    let cur_id = scheduler::current_thread_id();
    let mut sigmasks = THREAD_SIGMASKS.lock();
    let current_mask = *sigmasks.entry(cur_id).or_insert(0);

    if oldset_ptr != 0 {
        if oldset_ptr >= USER_SPACE_END || oldset_ptr + 8 > USER_SPACE_END {
            return (-(14i64)) as u64; // -EFAULT
        }
        unsafe {
            core::ptr::write_unaligned(oldset_ptr as *mut u64, current_mask);
        }
    }

    if set_ptr != 0 {
        if set_ptr >= USER_SPACE_END || set_ptr + 8 > USER_SPACE_END {
            return (-(14i64)) as u64;
        }
        let set_mask = unsafe { core::ptr::read_unaligned(set_ptr as *const u64) };
        let new_mask = match how {
            SIG_BLOCK => current_mask | set_mask,
            SIG_UNBLOCK => current_mask & !set_mask,
            SIG_SETMASK => set_mask,
            _ => return (-(22i64)) as u64,
        };
        // Битовая маска не может блокировать SIGKILL (бит 9) и SIGSTOP (бит 19)
        let filtered_mask = new_mask & !((1 << 9) | (1 << 19));
        sigmasks.insert(cur_id, filtered_mask);
    }

    0
}

pub fn sys_sigaltstack(ss_ptr: u64, old_ss_ptr: u64) -> u64 {
    let cur_id = scheduler::current_thread_id();
    let mut altstacks = THREAD_ALTSTACKS.lock();
    let current_ss = *altstacks.entry(cur_id).or_insert(StackT::default());

    if old_ss_ptr != 0 {
        if old_ss_ptr >= USER_SPACE_END || old_ss_ptr + core::mem::size_of::<StackT>() as u64 > USER_SPACE_END {
            return (-(14i64)) as u64; // -EFAULT
        }
        unsafe {
            core::ptr::write_unaligned(old_ss_ptr as *mut StackT, current_ss);
        }
    }

    if ss_ptr != 0 {
        if ss_ptr >= USER_SPACE_END || ss_ptr + core::mem::size_of::<StackT>() as u64 > USER_SPACE_END {
            return (-(14i64)) as u64;
        }
        let new_ss = unsafe { core::ptr::read_unaligned(ss_ptr as *const StackT) };
        altstacks.insert(cur_id, new_ss);
    }

    0
}
