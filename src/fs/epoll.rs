use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

use super::{FileHandle, EPOLLERR, EPOLLHUP};
use crate::task::scheduler;

pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_EPOLL_PWAIT: u64 = 281;
pub const SYS_EPOLL_CREATE1: u64 = 291;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

pub const EPOLL_CLOEXEC: u32 = 0x80000;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct EpollWatch {
    pub events: u32,
    pub data: u64,
}

pub struct EpollInstance {
    watches: Mutex<BTreeMap<u64, EpollWatch>>,
}

impl EpollInstance {
    pub fn new() -> Self {
        Self {
            watches: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn ctl(&self, op: u32, fd: u64, event_ptr: u64) -> Result<u64, i64> {
        let mut watches = self.watches.lock();

        match op {
            EPOLL_CTL_ADD => {
                if event_ptr == 0 || event_ptr >= 0x0000_8000_0000_0000 {
                    return Err(-14); // -EFAULT
                }
                let ev = unsafe { core::ptr::read_unaligned(event_ptr as *const EpollEvent) };
                if watches.contains_key(&fd) {
                    return Err(-17); // -EEXIST
                }
                watches.insert(fd, EpollWatch {
                    events: ev.events,
                    data: ev.data,
                });
                Ok(0)
            }
            EPOLL_CTL_DEL => {
                if watches.remove(&fd).is_some() {
                    Ok(0)
                } else {
                    Err(-2) // -ENOENT
                }
            }
            EPOLL_CTL_MOD => {
                if event_ptr == 0 || event_ptr >= 0x0000_8000_0000_0000 {
                    return Err(-14); // -EFAULT
                }
                let ev = unsafe { core::ptr::read_unaligned(event_ptr as *const EpollEvent) };
                if let Some(w) = watches.get_mut(&fd) {
                    w.events = ev.events;
                    w.data = ev.data;
                    Ok(0)
                } else {
                    Err(-2) // -ENOENT
                }
            }
            _ => Err(-22), // -EINVAL
        }
    }

    pub fn wait(&self, events_ptr: u64, maxevents: u32, timeout_ms: i32) -> Result<u64, i64> {
        if maxevents == 0 || maxevents > 1024 {
            return Err(-22); // -EINVAL
        }
        let total_bytes = (maxevents as u64) * core::mem::size_of::<EpollEvent>() as u64;
        if events_ptr == 0 || events_ptr + total_bytes > 0x0000_8000_0000_0000 {
            return Err(-14); // -EFAULT
        }

        let start_tick = crate::arch::x86_64::timer::ticks();
        let deadline = if timeout_ms >= 0 {
            // 100 Гц PIT: 1 тик = 10 мс
            let ticks = (timeout_ms as u64 + 9) / 10;
            Some(start_tick + ticks)
        } else {
            None
        };

        loop {
            let mut ready = Vec::new();

            {
                let watches = self.watches.lock();
                for (&fd, watch) in watches.iter() {
                    if let Some(handle) = crate::fs::get_file_handle(fd) {
                        let active = handle.poll_events();
                        let matched = active & (watch.events | EPOLLERR | EPOLLHUP);
                        if matched != 0 {
                            ready.push(EpollEvent {
                                events: matched,
                                data: watch.data,
                            });
                            if ready.len() >= maxevents as usize {
                                break;
                            }
                        }
                    }
                }
            }

            if !ready.is_empty() {
                let count = ready.len();
                let dst = events_ptr as *mut EpollEvent;
                for (i, ev) in ready.into_iter().enumerate() {
                    unsafe {
                        core::ptr::write_unaligned(dst.add(i), ev);
                    }
                }
                return Ok(count as u64);
            }

            // Нет готовых событий
            if timeout_ms == 0 {
                return Ok(0);
            }

            if let Some(dl) = deadline {
                if crate::arch::x86_64::timer::ticks() >= dl {
                    return Ok(0);
                }
            }

            // Передаем управление другим потокам и ждем активности (таймер, пайпы)
            scheduler::yield_now();
        }
    }
}

impl FileHandle for EpollInstance {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, i64> {
        Err(-9)
    }

    fn write(&self, _buf: &[u8]) -> Result<usize, i64> {
        Err(-9)
    }

    fn poll_events(&self) -> u32 {
        0
    }

    fn epoll_ctl(&self, op: u32, fd: u64, event_ptr: u64) -> Result<u64, i64> {
        self.ctl(op, fd, event_ptr)
    }

    fn epoll_wait(&self, events_ptr: u64, maxevents: u32, timeout_ms: i32) -> Result<u64, i64> {
        self.wait(events_ptr, maxevents, timeout_ms)
    }
}
