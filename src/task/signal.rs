use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use super::scheduler;
use super::thread::ThreadId;
use crate::arch::x86_64::syscall::SyscallRegisters;

pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_SIGALTSTACK: u64 = 131;

pub const NSIG: usize = 65;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct SigInfo {
    si_signo: i32,
    si_errno: i32,
    si_code: i32,
    _pad: i32,
    _fields: [u8; 112],
}

impl Default for SigInfo {
    fn default() -> Self {
        Self {
            si_signo: 0,
            si_errno: 0,
            si_code: 0,
            _pad: 0,
            _fields: [0; 112],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SigContext {
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rdi: u64,
    rsi: u64,
    rbp: u64,
    rbx: u64,
    rdx: u64,
    rax: u64,
    rcx: u64,
    rsp: u64,
    rip: u64,
    eflags: u64,
    cs: u16,
    gs: u16,
    fs: u16,
    ss: u16,
    err: u64,
    trapno: u64,
    oldmask: u64,
    cr2: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UContext {
    uc_flags: u64,
    uc_link: u64,
    uc_stack: StackT,
    uc_mcontext: SigContext,
    uc_sigmask: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RtSigFrame {
    pretcode: u64,
    info: SigInfo,
    uc: UContext,
}

const DEFAULT_ACTIONS: [SigAction; NSIG] = [SigAction {
    sa_handler: SIG_DFL,
    sa_flags: 0,
    sa_restorer: 0,
    sa_mask: 0,
}; NSIG];

static PROCESS_SIGACTIONS: Mutex<BTreeMap<u64, [SigAction; NSIG]>> = Mutex::new(BTreeMap::new());
static PROCESS_PENDING: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
static THREAD_SIGMASKS: Mutex<BTreeMap<ThreadId, u64>> = Mutex::new(BTreeMap::new());
static THREAD_ALTSTACKS: Mutex<BTreeMap<ThreadId, StackT>> = Mutex::new(BTreeMap::new());

static FOREGROUND_TGID: AtomicU64 = AtomicU64::new(0);

pub fn foreground_tgid() -> u64 {
    FOREGROUND_TGID.load(Ordering::Relaxed)
}

pub fn set_foreground_tgid(tgid: u64) {
    FOREGROUND_TGID.store(tgid, Ordering::SeqCst);
}

#[inline]
fn sig_bit(sig: u64) -> u64 {
    if sig >= 1 && sig <= 64 {
        1u64 << (sig - 1)
    } else {
        0
    }
}

#[inline]
fn is_default_ignore(sig: u64) -> bool {
    // По POSIX SIGCHLD (17), SIGCONT (18), SIGURG (23), SIGWINCH (28) по умолчанию игнорируются
    matches!(sig, 17 | 18 | 23 | 28)
}

pub fn raise_signal(tgid: u64, sig: u64) {
    let bit = sig_bit(sig);
    if tgid == 0 || bit == 0 {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut pending = PROCESS_PENDING.lock();
        let entry = pending.entry(tgid).or_insert(0);
        *entry |= bit;
    });
    scheduler::unblock_foreground(tgid);
}

pub fn raise_foreground_signal(sig: u64) {
    let fg = foreground_tgid();
    if fg != 0 {
        raise_signal(fg, sig);
    }
}

/// Вызывается при нажатии комбинации Ctrl+C с клавиатуры или поступлении VINTR из COM1
pub fn raise_sigint() {
    if !crate::task::syscall::fs::is_isig_enabled() {
        return;
    }
    raise_foreground_signal(super::syscall::signal::SIGINT);
}

pub fn clear_sigint() {
    let fg = foreground_tgid();
    let cur = crate::task::current_thread_tgid();
    let bit = sig_bit(super::syscall::signal::SIGINT);
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut pending = PROCESS_PENDING.lock();
        if let Some(mask) = pending.get_mut(&cur) {
            *mask &= !bit;
        }
        if fg != 0 && fg != cur {
            if let Some(mask) = pending.get_mut(&fg) {
                *mask &= !bit;
            }
        }
    });
}

pub fn is_sigint_pending() -> bool {
    let fg = foreground_tgid();
    let bit = sig_bit(super::syscall::signal::SIGINT);
    x86_64::instructions::interrupts::without_interrupts(|| {
        let pending = PROCESS_PENDING.lock();
        pending.get(&fg).map_or(false, |&m| (m & bit) != 0)
    })
}

pub fn is_sigint_pending_for_current() -> bool {
    let tgid = crate::task::current_thread_tgid();
    let tid = scheduler::current_thread_id();
    has_deliverable_signal_num(tgid, tid) == Some(super::syscall::signal::SIGINT)
}

pub fn has_deliverable_signal_for_current() -> bool {
    let tgid = crate::task::current_thread_tgid();
    let tid = scheduler::current_thread_id();
    has_deliverable_signal_num(tgid, tid).is_some()
}

pub fn reset_signals_on_exec(tgid: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(actions) = PROCESS_SIGACTIONS.lock().get_mut(&tgid) {
            // По POSIX execve сбрасывает перехваченные обработчики в SIG_DFL, но сохраняет SIG_IGN
            for act in actions.iter_mut() {
                if act.sa_handler != SIG_IGN {
                    *act = SigAction::default();
                }
            }
        }
        PROCESS_PENDING.lock().remove(&tgid);
    });
}

pub fn cleanup_process_signals(tgid: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        PROCESS_SIGACTIONS.lock().remove(&tgid);
        PROCESS_PENDING.lock().remove(&tgid);
    });
}

fn get_sigaction(tgid: u64, sig: u64) -> SigAction {
    if sig == 0 || sig >= NSIG as u64 {
        return SigAction::default();
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        PROCESS_SIGACTIONS
            .lock()
            .get(&tgid)
            .map(|a| a[sig as usize])
            .unwrap_or_default()
    })
}

fn has_deliverable_signal_num(tgid: u64, tid: ThreadId) -> Option<u64> {
    let mut pending_bits = x86_64::instructions::interrupts::without_interrupts(|| {
        *PROCESS_PENDING.lock().get(&tgid).unwrap_or(&0)
    });
    if pending_bits == 0 {
        return None;
    }
    let blocked = x86_64::instructions::interrupts::without_interrupts(|| {
        *THREAD_SIGMASKS.lock().get(&tid).unwrap_or(&0)
    });
    // SIGKILL (9) и SIGSTOP (19) не могут быть замаскированы
    let unblockable = sig_bit(9) | sig_bit(19);
    pending_bits &= !(blocked & !unblockable);

    while pending_bits != 0 {
        let bit_idx = pending_bits.trailing_zeros() as u64;
        let sig = bit_idx + 1;
        let bit = 1u64 << bit_idx;
        pending_bits &= !bit;

        let act = get_sigaction(tgid, sig);
        if act.sa_handler == SIG_IGN || (act.sa_handler == SIG_DFL && is_default_ignore(sig)) {
            x86_64::instructions::interrupts::without_interrupts(|| {
                if let Some(p) = PROCESS_PENDING.lock().get_mut(&tgid) {
                    *p &= !bit;
                }
            });
            continue;
        }
        return Some(sig);
    }
    None
}

/// Доставляет ожидающие сигналы текущему процессу перед возвратом из системного вызова в Ring 3.
pub fn deliver_pending_signals(regs: *mut SyscallRegisters, syscall_ret: u64) -> u64 {
    let tgid = crate::task::current_thread_tgid();
    let tid = scheduler::current_thread_id();

    let Some(sig) = has_deliverable_signal_num(tgid, tid) else {
        return syscall_ret;
    };

    let bit = sig_bit(sig);
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(p) = PROCESS_PENDING.lock().get_mut(&tgid) {
            *p &= !bit;
        }
    });

    let act = get_sigaction(tgid, sig);
    if act.sa_handler == SIG_IGN || (act.sa_handler == SIG_DFL && is_default_ignore(sig)) {
        return syscall_ret;
    }

    if act.sa_handler == SIG_DFL || act.sa_restorer == 0 || act.sa_handler >= USER_SPACE_END {
        if sig == super::syscall::signal::SIGINT && super::syscall::proc::is_current_interactive_shell() {
            crate::fs::handle::flush_stdin();
            crate::drivers::keyboard::clear_key_queue();
            crate::serial_println!("^C");
            crate::println!("^C");
            return syscall_ret;
        }
        crate::fs::handle::flush_stdin();
        crate::drivers::keyboard::clear_key_queue();
        if let Some(console) = crate::drivers::display::CONSOLE.lock().as_mut() {
            console.reset_terminal_modes();
        }
        crate::serial_println!("^C");
        crate::println!("^C");
        super::syscall::proc::sys_exit_group(128 + sig);
    }

    // Пользовательский обработчик сигнала: строим RtSigFrame на стеке Ring 3 по SysV x86_64 ABI
    let r = unsafe { &mut *regs };
    let frame_size = core::mem::size_of::<RtSigFrame>() as u64;
    // Отступаем 128 байт (x86_64 red zone) и выравниваем так, чтобы frame_addr % 16 == 8
    // (как после инструкции call перед входом в обработчик).
    let aligned_top = (r.user_rsp.saturating_sub(128).saturating_sub(frame_size)) & !0xFu64;
    let frame_addr = aligned_top.saturating_sub(8);
    if frame_addr == 0 || frame_addr + frame_size > USER_SPACE_END {
        super::syscall::proc::sys_exit_group(128 + sig);
    }

    let old_mask = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut masks = THREAD_SIGMASKS.lock();
        let old = *masks.get(&tid).unwrap_or(&0);
        let mut next = old | act.sa_mask;
        if (act.sa_flags & SA_NODEFER) == 0 {
            next |= bit;
        }
        next &= !(sig_bit(9) | sig_bit(19));
        masks.insert(tid, next);
        old
    });

    if (act.sa_flags & SA_RESETHAND) != 0 {
        x86_64::instructions::interrupts::without_interrupts(|| {
            if let Some(actions) = PROCESS_SIGACTIONS.lock().get_mut(&tgid) {
                actions[sig as usize] = SigAction::default();
            }
        });
    }

    let mut frame = RtSigFrame::default();
    frame.pretcode = act.sa_restorer;
    frame.info.si_signo = sig as i32;
    frame.uc.uc_sigmask = old_mask;
    frame.uc.uc_mcontext = SigContext {
        r8: r.r8,
        r9: r.r9,
        r10: r.r10,
        r11: r.user_rflags,
        r12: r.r12,
        r13: r.r13,
        r14: r.r14,
        r15: r.r15,
        rdi: r.rdi,
        rsi: r.rsi,
        rbp: r.rbp,
        rbx: r.rbx,
        rdx: r.rdx,
        rax: syscall_ret,
        rcx: r.user_rip,
        rsp: r.user_rsp,
        rip: r.user_rip,
        eflags: r.user_rflags,
        cs: 0x23,
        ss: 0x1b,
        oldmask: old_mask,
        ..Default::default()
    };

    unsafe {
        core::ptr::write_unaligned(frame_addr as *mut RtSigFrame, frame);
    }

    let info_offset = core::mem::offset_of!(RtSigFrame, info) as u64;
    let uc_offset = core::mem::offset_of!(RtSigFrame, uc) as u64;
    r.rdi = sig;
    r.rsi = frame_addr + info_offset;
    r.rdx = frame_addr + uc_offset;
    r.user_rip = act.sa_handler;
    r.user_rsp = frame_addr;

    0
}

pub fn sys_rt_sigreturn(regs: *mut SyscallRegisters) -> u64 {
    let r = unsafe { &mut *regs };
    // При возврате из обработчика инструкция ret сняла 8-байтный pretcode со стека
    let frame_addr = r.user_rsp.saturating_sub(8);
    let frame_size = core::mem::size_of::<RtSigFrame>() as u64;
    if frame_addr == 0 || frame_addr + frame_size > USER_SPACE_END {
        return (-(14i64)) as u64;
    }

    let frame = unsafe { core::ptr::read_unaligned(frame_addr as *const RtSigFrame) };
    let mc = &frame.uc.uc_mcontext;

    r.r8 = mc.r8;
    r.r9 = mc.r9;
    r.r10 = mc.r10;
    r.r12 = mc.r12;
    r.r13 = mc.r13;
    r.r14 = mc.r14;
    r.r15 = mc.r15;
    r.rdi = mc.rdi;
    r.rsi = mc.rsi;
    r.rbp = mc.rbp;
    r.rbx = mc.rbx;
    r.rdx = mc.rdx;
    r.user_rip = mc.rip;
    r.user_rsp = mc.rsp;
    r.user_rflags = (mc.eflags & 0x0020_0FFF) | 0x202;

    let tid = scheduler::current_thread_id();
    let restored_mask = frame.uc.uc_sigmask & !(sig_bit(9) | sig_bit(19));
    x86_64::instructions::interrupts::without_interrupts(|| {
        THREAD_SIGMASKS.lock().insert(tid, restored_mask);
    });

    mc.rax
}

pub fn sys_rt_sigaction(signum: u64, act_ptr: u64, oldact_ptr: u64, _sigsetsize: u64) -> u64 {
    if signum == 0 || signum >= (NSIG as u64) {
        return (-(22i64)) as u64; // -EINVAL
    }

    let tgid = crate::task::current_thread_tgid();
    let mut all_actions = PROCESS_SIGACTIONS.lock();
    let actions = all_actions.entry(tgid).or_insert(DEFAULT_ACTIONS);

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
