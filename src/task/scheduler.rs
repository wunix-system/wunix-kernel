use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;
use x86_64::instructions::interrupts;

use super::context;
use super::thread::{Thread, ThreadId, ThreadState};

pub struct Scheduler {
    pub(crate) current: Option<Box<Thread>>,
    pub(crate) ready_queue: VecDeque<Box<Thread>>,
    pub(crate) blocked_threads: BTreeMap<ThreadId, Box<Thread>>,
    // Отложенное освобождение завершенного потока.
    // Нельзя дропать стек потока, пока процессор физически исполняет код на нем!
    pub(crate) garbage: Option<Box<Thread>>,
}

static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

pub fn is_initialized() -> bool {
    interrupts::without_interrupts(|| SCHEDULER.lock().is_some())
}

pub fn has_ready_threads() -> bool {
    interrupts::without_interrupts(|| {
        SCHEDULER.lock().as_ref().map_or(false, |s| !s.ready_queue.is_empty())
    })
}

pub fn current_thread_id() -> ThreadId {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.id)
            .unwrap_or(ThreadId(0))
    })
}

pub fn current_thread_tgid() -> u64 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.tgid)
            .unwrap_or(1)
    })
}

pub fn current_thread_parent_tgid() -> u64 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.parent_tgid)
            .unwrap_or(1)
    })
}

pub fn current_thread_pgid() -> u64 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.pgid)
            .unwrap_or(1)
    })
}

pub fn set_current_thread_pgid(pgid: u64) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.pgid = pgid;
            }
        }
    });
}

pub fn current_thread_uid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.uid)
            .unwrap_or(0)
    })
}

pub fn current_thread_gid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.gid)
            .unwrap_or(0)
    })
}

pub fn current_thread_euid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.euid)
            .unwrap_or(0)
    })
}

pub fn current_thread_egid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.egid)
            .unwrap_or(0)
    })
}

pub fn current_thread_suid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.suid)
            .unwrap_or(0)
    })
}

pub fn current_thread_sgid() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.sgid)
            .unwrap_or(0)
    })
}

pub fn current_thread_umask() -> u32 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.umask)
            .unwrap_or(0o022)
    })
}

pub fn set_current_thread_uid(uid: u32, euid: u32, suid: u32) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.uid = uid;
                t.euid = euid;
                t.suid = suid;
            }
        }
    });
}

pub fn set_current_thread_gid(gid: u32, egid: u32, sgid: u32) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.gid = gid;
                t.egid = egid;
                t.sgid = sgid;
            }
        }
    });
}

pub fn set_current_thread_umask(umask: u32) -> u32 {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                let old = t.umask;
                t.umask = umask & 0o777;
                return old;
            }
        }
        0o022
    })
}

pub fn is_current_main_thread() -> bool {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.is_main)
            .unwrap_or(true)
    })
}

pub fn set_current_thread_tgid(tgid: u64) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.tgid = tgid;
            }
        }
    });
}

pub fn current_thread_kernel_stack_top() -> u64 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.kernel_stack_top)
            .unwrap_or(0)
    })
}

pub fn set_current_thread_kernel_stack_top(top: u64) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.kernel_stack_top = top;
            }
        }
    });
}

/// Удаляет из очередей планировщика все потоки группы процесса `tgid`, кроме `keep_tid`.
pub fn terminate_process_threads(tgid: u64, keep_tid: ThreadId) {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        if let Some(sched) = lock.as_mut() {
            sched.ready_queue.retain(|t| !(t.tgid == tgid && t.id != keep_tid));
            sched.blocked_threads.retain(|_, t| !(t.tgid == tgid && t.id != keep_tid));
        }
    });
}

pub fn set_current_fs_base(base: u64) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.fs_base = base;
            }
        }
    });
}

pub fn get_current_fs_base() -> u64 {
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .map(|t| t.fs_base)
            .unwrap_or(0)
    })
}

pub fn set_current_clear_child_tid(tid_ptr: u64) {
    interrupts::without_interrupts(|| {
        if let Some(sched) = SCHEDULER.lock().as_mut() {
            if let Some(t) = sched.current.as_mut() {
                t.clear_child_tid = tid_ptr;
            }
        }
    });
}

pub fn init() {
    super::syslib::init_syslib();

    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        if lock.is_some() {
            return;
        }

        let main_thread = Box::new(Thread::new_main());
        *lock = Some(Scheduler {
            current: Some(main_thread),
            ready_queue: VecDeque::new(),
            blocked_threads: BTreeMap::new(),
            garbage: None,
        });
    });
}

pub fn spawn(entry_fn: fn()) -> ThreadId {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = lock.as_mut().expect("scheduler: not initialized");
        let id = ThreadId::new();
        let thread = Box::new(Thread::new(id, entry_fn));
        sched.ready_queue.push_back(thread);
        id
    })
}

pub fn add_thread(thread: Box<Thread>) {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = lock.as_mut().expect("scheduler: not initialized");
        sched.ready_queue.push_back(thread);
    });
}

pub fn unblock_thread(id: ThreadId) {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        if let Some(sched) = lock.as_mut() {
            if let Some(mut thread) = sched.blocked_threads.remove(&id) {
                thread.state = ThreadState::Ready;
                thread.wait_deadline = None;
                sched.ready_queue.push_back(thread);
            }
        }
    });
}

/// Переводит текущий поток в состояние Blocked и переключает контекст на следующий готовый поток.
pub fn block_current(deadline: Option<u64>) {
    loop {
        if let Some(dl) = deadline {
            if crate::arch::x86_64::timer::ticks() >= dl {
                break;
            }
        }
        if crate::task::signal::is_sigint_pending()
            && !crate::task::syscall::proc::is_current_interactive_shell()
        {
            break;
        }

        let switch_data = interrupts::without_interrupts(|| {
            let mut lock = SCHEDULER.lock();
            let sched = match lock.as_mut() {
                Some(s) => s,
                None => return None,
            };

            sched.garbage = None;

            // Если в очереди готовности никого нет, ждем пробуждения от прерываний (таймер PIT)
            if sched.ready_queue.is_empty() {
                return None;
            }

            super::reset_timeslice();

            let mut prev = sched.current.take().expect("scheduler: no current thread to block");
            let cur_fs = unsafe { crate::arch::x86_64::syscall::read_fs_base() };
            prev.fs_base = cur_fs;
            prev.state = ThreadState::Blocked;
            prev.wait_deadline = deadline;
            let prev_id = prev.id;
            sched.blocked_threads.insert(prev_id, prev);

            let mut next = sched.ready_queue.pop_front().expect("scheduler: ready queue empty");
            next.state = ThreadState::Running;

            if next.kernel_stack_top != 0 {
                crate::arch::x86_64::syscall::set_kernel_stack(next.kernel_stack_top);
                crate::arch::x86_64::gdt::set_tss_rsp0(next.kernel_stack_top);
            }
            unsafe {
                crate::arch::x86_64::syscall::write_fs_base(next.fs_base);
            }

            let prev_rsp = &mut sched.blocked_threads.get_mut(&prev_id).unwrap().rsp as *mut *mut u8;
            let next_rsp = next.rsp as *const u8;
            sched.current = Some(next);

            Some((prev_rsp, next_rsp))
        });

        if let Some((prev_rsp, next_rsp)) = switch_data {
            unsafe {
                context::context_switch(prev_rsp, next_rsp);
            }
            break;
        } else {
            // Все потоки заблокированы — засыпаем до следующего прерывания таймера
            x86_64::instructions::interrupts::enable();
            x86_64::instructions::hlt();
            check_timeouts();
            if let Some(dl) = deadline {
                if crate::arch::x86_64::timer::ticks() >= dl {
                    break;
                }
            }
            if crate::task::signal::is_sigint_pending()
                && !crate::task::syscall::proc::is_current_interactive_shell()
            {
                break;
            }
        }
    }
}

pub fn check_timeouts() {
    interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        if let Some(sched) = lock.as_mut() {
            let now = crate::arch::x86_64::timer::ticks();
            let sigint = crate::task::signal::is_sigint_pending()
                && !crate::task::syscall::proc::is_current_interactive_shell();
            let mut expired = alloc::vec::Vec::new();
            for (id, thread) in &sched.blocked_threads {
                if sigint {
                    // При получении сигнала SIGINT немедленно будим заблокированный дочерний процесс,
                    // не дожидаясь истечения таймаутов (nanosleep, poll, futex)
                    expired.push(*id);
                } else if let Some(deadline) = thread.wait_deadline {
                    if now >= deadline {
                        expired.push(*id);
                    }
                }
            }
            for id in expired {
                if let Some(mut thread) = sched.blocked_threads.remove(&id) {
                    thread.state = ThreadState::Ready;
                    thread.wait_deadline = None;
                    sched.ready_queue.push_back(thread);
                }
            }
        }
    });
}

pub fn yield_now() {
    let switch_data = interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = match lock.as_mut() {
            Some(s) => s,
            None => return None,
        };

        // Освобождаем память завершенного ранее потока: сейчас мы исполняемся на стеке текущего потока,
        // поэтому старый стек можно безопасно вернуть аллокатору кучи.
        sched.garbage = None;

        if sched.ready_queue.is_empty() {
            return None;
        }

        super::reset_timeslice();

        let mut prev = sched.current.take().expect("scheduler: no current thread");
        let cur_fs = unsafe { crate::arch::x86_64::syscall::read_fs_base() };
        prev.fs_base = cur_fs;
        prev.state = ThreadState::Ready;
        sched.ready_queue.push_back(prev);

        let mut next = sched.ready_queue.pop_front().expect("scheduler: ready queue empty");
        next.state = ThreadState::Running;

        if next.kernel_stack_top != 0 {
            crate::arch::x86_64::syscall::set_kernel_stack(next.kernel_stack_top);
            crate::arch::x86_64::gdt::set_tss_rsp0(next.kernel_stack_top);
        }
        unsafe {
            crate::arch::x86_64::syscall::write_fs_base(next.fs_base);
        }

        // Извлекаем стабильный указатель на поле rsp внутри Box<Thread> в очереди
        let prev_rsp = &mut sched.ready_queue.back_mut().unwrap().rsp as *mut *mut u8;
        let next_rsp = next.rsp as *const u8;
        sched.current = Some(next);

        Some((prev_rsp, next_rsp))
    });

    if let Some((prev_rsp, next_rsp)) = switch_data {
        unsafe {
            context::context_switch(prev_rsp, next_rsp);
        }
    }
}

pub fn exit() -> ! {
    let next_rsp = interrupts::without_interrupts(|| {
        let mut lock = SCHEDULER.lock();
        let sched = lock.as_mut().expect("scheduler: not initialized on exit");

        sched.garbage = None;

        let prev = sched.current.take().expect("scheduler: no current thread on exit");

        // Обработка clear_child_tid: при завершении потока ядро записывает 0 и пробуждает futex
        let clear_tid = prev.clear_child_tid;
        if clear_tid != 0 && clear_tid < 0x0000_8000_0000_0000 {
            unsafe {
                core::ptr::write_volatile(clear_tid as *mut u32, 0);
            }
            drop(lock);
            let _ = super::futex::futex_wake(clear_tid, 1);
            lock = SCHEDULER.lock();
            let sched_reacquired = lock.as_mut().expect("scheduler: reacquire on exit");
            return do_exit_switch(sched_reacquired, prev);
        }

        do_exit_switch(sched, prev)
    });

    let mut dummy_rsp: *mut u8 = core::ptr::null_mut();
    unsafe {
        context::context_switch(&mut dummy_rsp, next_rsp);
    }

    crate::halt_loop();
}

fn do_exit_switch(sched: &mut Scheduler, mut prev: Box<Thread>) -> *const u8 {
    prev.state = ThreadState::Terminated;
    sched.garbage = Some(prev);

    while sched.ready_queue.is_empty() && !sched.blocked_threads.is_empty() {
        x86_64::instructions::interrupts::enable();
        x86_64::instructions::hlt();
        check_timeouts();
    }

    let mut next = sched.ready_queue.pop_front().expect("scheduler: no runnable threads on exit");
    next.state = ThreadState::Running;

    if next.kernel_stack_top != 0 {
        crate::arch::x86_64::syscall::set_kernel_stack(next.kernel_stack_top);
        crate::arch::x86_64::gdt::set_tss_rsp0(next.kernel_stack_top);
    }
    unsafe {
        crate::arch::x86_64::syscall::write_fs_base(next.fs_base);
    }

    let next_rsp = next.rsp as *const u8;
    sched.current = Some(next);

    super::reset_timeslice();
    next_rsp
}
