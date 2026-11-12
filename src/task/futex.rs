use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

use super::scheduler;
use super::thread::ThreadId;

pub const SYS_FUTEX: u64 = 202;

// Команды операций над футексами Linux ABI
pub const FUTEX_WAIT: u64 = 0;
pub const FUTEX_WAKE: u64 = 1;
pub const FUTEX_FD: u64 = 2;
pub const FUTEX_REQUEUE: u64 = 3;
pub const FUTEX_CMP_REQUEUE: u64 = 4;
pub const FUTEX_WAKE_OP: u64 = 5;
pub const FUTEX_LOCK_PI: u64 = 6;
pub const FUTEX_UNLOCK_PI: u64 = 7;
pub const FUTEX_TRYLOCK_PI: u64 = 8;
pub const FUTEX_WAIT_BITSET: u64 = 9;
pub const FUTEX_WAKE_BITSET: u64 = 10;
pub const FUTEX_PRIVATE_FLAG: u64 = 128;
pub const FUTEX_CLOCK_REALTIME: u64 = 256;

pub const FUTEX_CMD_MASK: u64 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct FutexWaitNode {
    pub thread_id: ThreadId,
    pub deadline_ticks: Option<u64>,
    pub bitset: u32,
}

// Глобальная потокобезопасная очередь ожидающих потоков по виртуальному адресу футекса
static FUTEX_QUEUES: Mutex<BTreeMap<u64, VecDeque<FutexWaitNode>>> = Mutex::new(BTreeMap::new());

fn check_futex_uaddr(uaddr: u64) -> Result<(), i64> {
    if uaddr == 0 || uaddr >= USER_SPACE_END || uaddr % 4 != 0 {
        Err(-14) // -EFAULT
    } else {
        Ok(())
    }
}

fn compute_deadline_ticks(
    timeout_ptr: u64,
    is_absolute: bool,
    use_realtime: bool,
) -> Result<Option<u64>, i64> {
    if timeout_ptr == 0 {
        return Ok(None);
    }
    if timeout_ptr >= USER_SPACE_END {
        return Err(-14); // -EFAULT
    }
    let ts = unsafe { core::ptr::read_unaligned(timeout_ptr as *const Timespec) };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return Err(-22); // -EINVAL
    }

    let now_ticks = crate::arch::x86_64::timer::ticks();
    if is_absolute {
        let (cur_sec, cur_nsec) = if use_realtime {
            crate::task::syscall::proc::get_realtime_epoch()
        } else {
            ((now_ticks / 100) as i64, ((now_ticks % 100) * 10_000_000) as i64)
        };
        let delta_ns = (ts.tv_sec - cur_sec)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec - cur_nsec);
        if delta_ns <= 0 {
            return Err(-110); // -ETIMEDOUT
        }
        let delta_ticks = (((delta_ns as u64) + 9_999_999) / 10_000_000).max(1);
        Ok(Some(now_ticks.saturating_add(delta_ticks)))
    } else {
        // 100 Гц PIT: 1 тик = 10 миллисекунд
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            return Err(-110);
        }
        let ticks_from_sec = (ts.tv_sec as u64).saturating_mul(100);
        let ticks_from_nsec = (ts.tv_nsec as u64 + 9_999_999) / 10_000_000;
        let delta = ticks_from_sec.saturating_add(ticks_from_nsec).max(1);
        Ok(Some(now_ticks.saturating_add(delta)))
    }
}

/// Блокирует вызывающий поток, если 32-битное значение по адресу `uaddr` равно `val`.
/// Поддерживает как относительный `FUTEX_WAIT`, так и абсолютный `FUTEX_WAIT_BITSET`.
pub fn futex_wait_bitset(
    uaddr: u64,
    val: u32,
    timeout_ptr: u64,
    bitset: u32,
    is_absolute: bool,
    use_realtime: bool,
) -> Result<u64, i64> {
    check_futex_uaddr(uaddr)?;
    if bitset == 0 {
        return Err(-22); // -EINVAL
    }

    // Проверяем ожидаемое значение до расчёта таймаута, чтобы при гонке вернуть -EAGAIN, а не -ETIMEDOUT
    let val_now = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if val_now != val {
        return Err(-11); // -EAGAIN
    }

    let deadline_ticks = compute_deadline_ticks(timeout_ptr, is_absolute, use_realtime)?;
    let cur_id = scheduler::current_thread_id();

    {
        let mut queues = FUTEX_QUEUES.lock();
        queues
            .entry(uaddr)
            .or_insert_with(VecDeque::new)
            .push_back(FutexWaitNode {
                thread_id: cur_id,
                deadline_ticks,
                bitset,
            });
    }

    scheduler::block_current(deadline_ticks);

    // Если поток всё ещё в очереди после возврата — его не разбудили через WAKE, значит сработал таймаут
    let mut queues = FUTEX_QUEUES.lock();
    let mut still_queued = false;
    if let Some(q) = queues.get_mut(&uaddr) {
        let before = q.len();
        q.retain(|node| node.thread_id != cur_id);
        still_queued = q.len() < before;
        if q.is_empty() {
            queues.remove(&uaddr);
        }
    }

    if still_queued && deadline_ticks.is_some() {
        return Err(-110); // -ETIMEDOUT
    }

    Ok(0)
}

/// Пробуждает до `count` потоков на `uaddr`, чья маска пересекается с `bitset`.
pub fn futex_wake_bitset(uaddr: u64, count: u32, bitset: u32) -> Result<u64, i64> {
    check_futex_uaddr(uaddr)?;
    if bitset == 0 {
        return Err(-22); // -EINVAL
    }
    if count == 0 {
        return Ok(0);
    }

    let mut woken = 0u64;
    let mut queues = FUTEX_QUEUES.lock();
    if let Some(q) = queues.get_mut(&uaddr) {
        let mut remaining = VecDeque::with_capacity(q.len());
        while let Some(node) = q.pop_front() {
            if woken < (count as u64) && (node.bitset & bitset) != 0 {
                scheduler::unblock_thread(node.thread_id);
                woken += 1;
            } else {
                remaining.push_back(node);
            }
        }
        if remaining.is_empty() {
            queues.remove(&uaddr);
        } else {
            *q = remaining;
        }
    }

    Ok(woken)
}

pub fn futex_wake(uaddr: u64, count: u32) -> Result<u64, i64> {
    futex_wake_bitset(uaddr, count, u32::MAX)
}

fn futex_requeue(
    uaddr: u64,
    nr_wake: u64,
    nr_requeue: usize,
    uaddr2: u64,
    expected: Option<u32>,
) -> Result<u64, i64> {
    check_futex_uaddr(uaddr)?;
    check_futex_uaddr(uaddr2)?;

    if let Some(val) = expected {
        let val_now = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
        if val_now != val {
            return Err(-11); // -EAGAIN
        }
    }

    let mut woken = 0u64;
    let mut queues = FUTEX_QUEUES.lock();
    let mut to_move = VecDeque::new();
    if let Some(q) = queues.get_mut(&uaddr) {
        while woken < nr_wake {
            if let Some(node) = q.pop_front() {
                scheduler::unblock_thread(node.thread_id);
                woken += 1;
            } else {
                break;
            }
        }
        while to_move.len() < nr_requeue {
            if let Some(node) = q.pop_front() {
                to_move.push_back(node);
            } else {
                break;
            }
        }
        if q.is_empty() {
            queues.remove(&uaddr);
        }
    }
    if !to_move.is_empty() {
        queues
            .entry(uaddr2)
            .or_insert_with(VecDeque::new)
            .extend(to_move);
    }
    Ok(woken)
}

fn futex_wake_op(uaddr: u64, nr_wake: u32, nr_wake2: u32, uaddr2: u64, op_spec: u32) -> Result<u64, i64> {
    check_futex_uaddr(uaddr)?;
    check_futex_uaddr(uaddr2)?;

    let op = (op_spec >> 28) & 0x0f;
    let cmp = (op_spec >> 24) & 0x0f;
    // Знаковые 12-битные поля по стандарту linux/futex.h
    let mut oparg = ((op_spec << 8) as i32) >> 20;
    let cmparg = ((op_spec << 20) as i32) >> 20;
    if (op & 0x08) != 0 {
        oparg = 1i32.wrapping_shl((oparg as u32) & 31);
    }

    let atomic_ref = unsafe { &*(uaddr2 as *const core::sync::atomic::AtomicI32) };
    let oldval = loop {
        let cur = atomic_ref.load(core::sync::atomic::Ordering::SeqCst);
        let new = match op & 0x07 {
            0 => oparg,                 // FUTEX_OP_SET
            1 => cur.wrapping_add(oparg), // FUTEX_OP_ADD
            2 => cur | oparg,           // FUTEX_OP_OR
            3 => cur & !oparg,          // FUTEX_OP_ANDN
            4 => cur ^ oparg,           // FUTEX_OP_XOR
            _ => return Err(-38),       // -ENOSYS
        };
        if atomic_ref
            .compare_exchange_weak(
                cur,
                new,
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            break cur;
        }
    };

    let mut woken = futex_wake_bitset(uaddr, nr_wake, u32::MAX)?;
    let cond = match cmp {
        0 => oldval == cmparg, // FUTEX_OP_CMP_EQ
        1 => oldval != cmparg, // FUTEX_OP_CMP_NE
        2 => oldval < cmparg,  // FUTEX_OP_CMP_LT
        3 => oldval <= cmparg, // FUTEX_OP_CMP_LE
        4 => oldval > cmparg,  // FUTEX_OP_CMP_GT
        5 => oldval >= cmparg, // FUTEX_OP_CMP_GE
        _ => return Err(-38),
    };
    if cond {
        woken += futex_wake_bitset(uaddr2, nr_wake2, u32::MAX)?;
    }
    Ok(woken)
}

#[inline]
fn res_to_syscall(res: Result<u64, i64>) -> u64 {
    match res {
        Ok(v) => v,
        Err(err) => (-(err.abs())) as u64,
    }
}

/// Диспетчер системного вызова SYS_FUTEX (202)
pub fn sys_futex(
    uaddr: u64,
    op: u64,
    val: u64,
    timeout_ptr: u64,
    uaddr2: u64,
    val3: u64,
) -> u64 {
    let cmd = op & FUTEX_CMD_MASK;
    let use_realtime = (op & FUTEX_CLOCK_REALTIME) != 0;

    match cmd {
        FUTEX_WAIT => res_to_syscall(futex_wait_bitset(
            uaddr,
            val as u32,
            timeout_ptr,
            u32::MAX,
            false,
            use_realtime,
        )),
        FUTEX_WAIT_BITSET => res_to_syscall(futex_wait_bitset(
            uaddr,
            val as u32,
            timeout_ptr,
            val3 as u32,
            true,
            use_realtime,
        )),
        FUTEX_WAKE => res_to_syscall(futex_wake_bitset(uaddr, val as u32, u32::MAX)),
        FUTEX_WAKE_BITSET => res_to_syscall(futex_wake_bitset(uaddr, val as u32, val3 as u32)),
        FUTEX_REQUEUE => res_to_syscall(futex_requeue(
            uaddr,
            val,
            timeout_ptr as usize,
            uaddr2,
            None,
        )),
        FUTEX_CMP_REQUEUE => res_to_syscall(futex_requeue(
            uaddr,
            val,
            timeout_ptr as usize,
            uaddr2,
            Some(val3 as u32),
        )),
        FUTEX_WAKE_OP => res_to_syscall(futex_wake_op(
            uaddr,
            val as u32,
            timeout_ptr as u32,
            uaddr2,
            val3 as u32,
        )),
        _ => {
            crate::serial_println!("[futex] unhandled cmd={} (op={:#x})", cmd, op);
            (-(38i64)) as u64 // -ENOSYS
        }
    }
}
