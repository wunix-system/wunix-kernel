use crate::fs::get_file_handle;
use super::{validate_user_buffer, EFAULT, EINTR, EINVAL};

pub const POLLIN: i16 = 0x0001;
pub const POLLPRI: i16 = 0x0002;
pub const POLLOUT: i16 = 0x0004;
pub const POLLERR: i16 = 0x0008;
pub const POLLHUP: i16 = 0x0010;
pub const POLLNVAL: i16 = 0x0020;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub fn sys_poll(fds_ptr: u64, nfds: u64, timeout_ms: i64) -> u64 {
    if nfds == 0 {
        if timeout_ms > 0 {
            let ticks = (timeout_ms as u64 + 9) / 10;
            let deadline = crate::arch::x86_64::timer::ticks().saturating_add(ticks);
            crate::task::block_current(Some(deadline));
            if crate::task::signal::is_sigint_pending_for_current() {
                return (-(EINTR as i64)) as u64;
            }
        }
        return 0;
    }

    let byte_len = match nfds.checked_mul(core::mem::size_of::<PollFd>() as u64) {
        Some(l) => l,
        None => return (-(EINVAL as i64)) as u64,
    };

    if !validate_user_buffer(fds_ptr, byte_len) {
        return (-(EFAULT as i64)) as u64;
    }

    let slice = unsafe {
        core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds as usize)
    };

    let start_ticks = crate::arch::x86_64::timer::ticks();
    let delta = if timeout_ms > 0 {
        (timeout_ms as u64 + 9) / 10
    } else {
        0
    };
    let deadline = start_ticks.saturating_add(delta);

    loop {
        if crate::task::signal::is_sigint_pending_for_current() {
            return (-(EINTR as i64)) as u64;
        }

        // Опрашиваем сетевой стек для обновления статусов сетевых сокетов
        crate::net::poll();

        let mut ready_count = 0u64;

        for pfd in slice.iter_mut() {
            pfd.revents = 0;
            if pfd.fd < 0 {
                continue;
            }

            match get_file_handle(pfd.fd as u64) {
                Some(handle) => {
                    let ev = handle.poll_events() as i16;
                    let matched = ev & pfd.events;
                    let errors = ev & (POLLERR | POLLHUP);
                    pfd.revents = matched | errors;
                    if pfd.revents != 0 {
                        ready_count += 1;
                    }
                }
                None => {
                    pfd.revents = POLLNVAL;
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 || timeout_ms == 0 {
            return ready_count;
        }

        if timeout_ms > 0 && crate::arch::x86_64::timer::ticks() >= deadline {
            return 0;
        }

        // Переводим поток в состояние ожидания на 1 квант системного таймера (10 мс),
        // чтобы избежать 100% холостой загрузки процессора (busy loop).
        let now = crate::arch::x86_64::timer::ticks();
        let sleep_until = if timeout_ms > 0 {
            Some(now.saturating_add(1).min(deadline))
        } else {
            Some(now.saturating_add(1))
        };
        crate::task::block_current(sleep_until);
    }
}

pub fn sys_select(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
) -> u64 {
    if nfds > 1024 {
        return (-(EINVAL as i64)) as u64;
    }

    let words = ((nfds + 63) / 64) as usize;
    let byte_len = (words * 8) as u64;

    if readfds_ptr != 0 && !validate_user_buffer(readfds_ptr, byte_len) {
        return (-(EFAULT as i64)) as u64;
    }
    if writefds_ptr != 0 && !validate_user_buffer(writefds_ptr, byte_len) {
        return (-(EFAULT as i64)) as u64;
    }
    if exceptfds_ptr != 0 && !validate_user_buffer(exceptfds_ptr, byte_len) {
        return (-(EFAULT as i64)) as u64;
    }

    let timeout_ms = if timeout_ptr != 0 {
        if !validate_user_buffer(timeout_ptr, 16) {
            return (-(EFAULT as i64)) as u64;
        }
        let ts = unsafe { core::ptr::read_unaligned(timeout_ptr as *const crate::task::futex::Timespec) };
        (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64) / 1_000_000
    } else {
        -1
    };

    let start_ticks = crate::arch::x86_64::timer::ticks();
    let delta = if timeout_ms > 0 {
        (timeout_ms as u64 + 9) / 10
    } else {
        0
    };
    let deadline = start_ticks.saturating_add(delta);

    loop {
        if crate::task::signal::is_sigint_pending_for_current() {
            return (-(EINTR as i64)) as u64;
        }

        crate::net::poll();

        let mut ready_count = 0u64;

        let mut in_read = [0u64; 16];
        let mut in_write = [0u64; 16];
        let mut out_read = [0u64; 16];
        let mut out_write = [0u64; 16];

        if readfds_ptr != 0 {
            for i in 0..words {
                in_read[i] = unsafe { *(readfds_ptr as *const u64).add(i) };
            }
        }
        if writefds_ptr != 0 {
            for i in 0..words {
                in_write[i] = unsafe { *(writefds_ptr as *const u64).add(i) };
            }
        }

        for fd in 0..nfds {
            let word_idx = (fd / 64) as usize;
            let bit_mask = 1u64 << (fd % 64);

            let check_read = (in_read[word_idx] & bit_mask) != 0;
            let check_write = (in_write[word_idx] & bit_mask) != 0;

            if !check_read && !check_write {
                continue;
            }

            if let Some(handle) = get_file_handle(fd) {
                let ev = handle.poll_events();
                if check_read && (ev & crate::fs::EPOLLIN) != 0 {
                    out_read[word_idx] |= bit_mask;
                    ready_count += 1;
                }
                if check_write && (ev & crate::fs::EPOLLOUT) != 0 {
                    out_write[word_idx] |= bit_mask;
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 || timeout_ms == 0 {
            if readfds_ptr != 0 {
                for i in 0..words {
                    unsafe { *(readfds_ptr as *mut u64).add(i) = out_read[i] };
                }
            }
            if writefds_ptr != 0 {
                for i in 0..words {
                    unsafe { *(writefds_ptr as *mut u64).add(i) = out_write[i] };
                }
            }
            if exceptfds_ptr != 0 {
                for i in 0..words {
                    unsafe { *(exceptfds_ptr as *mut u64).add(i) = 0 };
                }
            }
            return ready_count;
        }

        if timeout_ms > 0 && crate::arch::x86_64::timer::ticks() >= deadline {
            return 0;
        }

        let now = crate::arch::x86_64::timer::ticks();
        let sleep_until = if timeout_ms > 0 {
            Some(now.saturating_add(1).min(deadline))
        } else {
            Some(now.saturating_add(1))
        };
        crate::task::block_current(sleep_until);
    }
}

pub fn sys_ppoll(
    fds_ptr: u64,
    nfds: u64,
    timeout_ptr: u64,
    _sigmask_ptr: u64,
    _sigsetsize: u64,
) -> u64 {
    let timeout_ms = if timeout_ptr != 0 {
        if !validate_user_buffer(timeout_ptr, 16) {
            return (-(EFAULT as i64)) as u64;
        }
        let ts = unsafe { core::ptr::read_unaligned(timeout_ptr as *const crate::task::futex::Timespec) };
        (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64) / 1_000_000
    } else {
        -1
    };
    sys_poll(fds_ptr, nfds, timeout_ms)
}

pub fn sys_pselect6(
    nfds: u64,
    readfds: u64,
    writefds: u64,
    exceptfds: u64,
    timeout_ptr: u64,
    _sigmask_ptr: u64,
) -> u64 {
    sys_select(nfds, readfds, writefds, exceptfds, timeout_ptr)
}
