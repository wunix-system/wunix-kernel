pub mod abi;
pub mod context;
pub mod darwin;
pub mod elf;
pub mod futex;
pub mod pe;
pub mod ring3_test;
pub mod scheduler;
pub mod signal;
pub mod syscall;
pub mod syslib;
pub mod thread;
pub mod win_syscall;

pub use abi::AbiPersonality;
pub use scheduler::{
    active_process_count, add_thread, block_current, check_timeouts, current_thread_abi, current_thread_egid, current_thread_euid,
    current_thread_exe_path, current_thread_gid, current_thread_id, current_thread_kernel_stack_top, current_thread_name,
    current_thread_owns_cr3, current_thread_parent_tgid, current_thread_pgid, current_thread_sgid, current_thread_suid,
    current_thread_tgid, current_thread_uid, current_thread_umask, exit, get_current_fs_base,
    has_child_process, has_ready_threads, init, is_current_main_thread, is_current_thread_fork_child,
    is_current_thread_sync_child, is_initialized, kill_process_threads, process_exists,
    set_current_clear_child_tid, set_current_fs_base, set_current_thread_abi,
    set_current_thread_cr3, set_current_thread_exe_path, set_current_thread_fork_child,
    set_current_thread_gid, set_current_thread_kernel_stack_top, set_current_thread_name,
    set_current_thread_owns_cr3, set_current_thread_pgid, set_current_thread_sync_child,
    set_current_thread_tgid, set_current_thread_uid, set_current_thread_umask, spawn,
    terminate_process_threads, thread_exists, unblock_thread, yield_now,
};
pub use syslib::{
    init_syslib, map_syslib_to_user_space, resolve_symbol_vaddr, AT_SYSINFO_EHDR, SYSLIB_INFO_VADDR,
};
pub use thread::{Thread, ThreadId, ThreadState, STACK_SIZE};

use core::sync::atomic::{AtomicU64, Ordering};

// Квант времени по умолчанию: 5 тиков системного таймера (при частоте PIT 100 Гц = 50 мс)
pub const TIMESLICE_TICKS: u64 = 5;

static CURRENT_TIMESLICE: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn reset_timeslice() {
    CURRENT_TIMESLICE.store(0, Ordering::Relaxed);
}

pub fn is_current_thread_foreground() -> bool {
    let cur_tgid = current_thread_tgid();
    let fg_tgid = signal::foreground_tgid();
    is_current_thread_sync_child() || (fg_tgid != 0 && cur_tgid == fg_tgid)
}

pub fn is_foreground_sync_child_active() -> bool {
    let depth = crate::arch::x86_64::syscall::current_child_stack_depth();
    depth > 1 && !crate::task::syscall::proc::is_current_interactive_shell()
}

pub fn on_timer_tick(is_user: bool) {
    if !is_initialized() {
        return;
    }

    // Проверяем дедлайны спящих потоков (futex, nanosleep, epoll)
    scheduler::check_timeouts();

    // Вытеснение по кванту времени разрешено только при возврате в Ring 3 (пользовательский режим).
    // Асинхронное переключение контекста внутри Ring 0 способно нарушить атомарность
    // критических секций ядра под спинлоками и повредить фрейм возврата прерывания.
    if !is_user {
        return;
    }

    let ticks = CURRENT_TIMESLICE.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks >= TIMESLICE_TICKS {
        CURRENT_TIMESLICE.store(0, Ordering::Relaxed);
        if has_ready_threads() {
            yield_now();
        }
    }
}
