use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use super::{FileHandle, EPOLLIN, EPOLLOUT, O_NONBLOCK};
use crate::task::scheduler;
use crate::task::thread::ThreadId;

pub const EFD_SEMAPHORE: u32 = 1;
pub const EFD_NONBLOCK: u32 = 0o0004000;
pub const EFD_CLOEXEC: u32 = 0o2000000;

pub struct EventFdHandle {
    counter: Mutex<u64>,
    flags: AtomicU32,
    waiting_readers: Mutex<alloc::vec::Vec<ThreadId>>,
}

impl EventFdHandle {
    pub fn new(initval: u32, flags: u32) -> Self {
        Self {
            counter: Mutex::new(initval as u64),
            flags: AtomicU32::new(flags),
            waiting_readers: Mutex::new(alloc::vec::Vec::new()),
        }
    }
}

impl FileHandle for EventFdHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.len() < 8 {
            return Err(-22); // -EINVAL
        }

        loop {
            let mut val = self.counter.lock();
            if *val > 0 {
                let to_return = if (self.flags.load(Ordering::Relaxed) & EFD_SEMAPHORE) != 0 {
                    *val -= 1;
                    1u64
                } else {
                    let cur = *val;
                    *val = 0;
                    cur
                };
                buf[..8].copy_from_slice(&to_return.to_ne_bytes());
                return Ok(8);
            }

            if (self.flags.load(Ordering::Relaxed) & (EFD_NONBLOCK | O_NONBLOCK)) != 0 {
                return Err(-11); // -EAGAIN
            }

            let cur_id = scheduler::current_thread_id();
            self.waiting_readers.lock().push(cur_id);
            drop(val);

            scheduler::block_current(None);
        }
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        if buf.len() < 8 {
            return Err(-22); // -EINVAL
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&buf[..8]);
        let add_val = u64::from_ne_bytes(bytes);
        if add_val == u64::MAX {
            return Err(-22); // -EINVAL
        }

        let mut val = self.counter.lock();
        if u64::MAX - *val <= add_val {
            if (self.flags.load(Ordering::Relaxed) & (EFD_NONBLOCK | O_NONBLOCK)) != 0 {
                return Err(-11);
            }
            return Err(-11);
        }

        *val += add_val;

        let readers = core::mem::take(&mut *self.waiting_readers.lock());
        drop(val);
        for tid in readers {
            scheduler::unblock_thread(tid);
        }

        Ok(8)
    }

    fn poll_events(&self) -> u32 {
        let val = *self.counter.lock();
        let mut ev = 0;
        if val > 0 {
            ev |= EPOLLIN;
        }
        if val < u64::MAX - 1 {
            ev |= EPOLLOUT;
        }
        ev
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }
}
