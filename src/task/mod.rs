pub mod context;
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

pub use scheduler::{
    add_thread, block_current, check_timeouts, current_thread_egid, current_thread_euid,
    current_thread_gid, current_thread_id, current_thread_kernel_stack_top,
    current_thread_parent_tgid, current_thread_pgid, current_thread_sgid, current_thread_suid,
    current_thread_tgid, current_thread_uid, current_thread_umask, exit, get_current_fs_base,
    has_ready_threads, init, is_current_main_thread, is_initialized, set_current_clear_child_tid,
    set_current_fs_base, set_current_thread_gid, set_current_thread_kernel_stack_top,
    set_current_thread_pgid, set_current_thread_tgid, set_current_thread_uid,
    set_current_thread_umask, spawn, terminate_process_threads, unblock_thread, yield_now,
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

pub fn on_timer_tick() {
    if !is_initialized() {
        return;
    }

    // Проверяем дедлайны спящих потоков (futex, nanosleep, epoll)
    scheduler::check_timeouts();

    let ticks = CURRENT_TIMESLICE.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks >= TIMESLICE_TICKS {
        CURRENT_TIMESLICE.store(0, Ordering::Relaxed);
        yield_now();
    }
}
