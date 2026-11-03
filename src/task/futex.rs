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
}

// Глобальная потокобезопасная очередь ожидающих потоков по виртуальному адресу футекса
static FUTEX_QUEUES: Mutex<BTreeMap<u64, VecDeque<FutexWaitNode>>> = Mutex::new(BTreeMap::new());

/// Блокирует вызывающий поток, если 32-битное значение по адресу `uaddr` равно `val`.
/// Возвращает 0 при успешном пробуждении через WAKE, либо код ошибки.
pub fn futex_wait(uaddr: u64, val: u32, timeout_ptr: u64) -> Result<u64, i64> {
    if uaddr >= USER_SPACE_END || uaddr % 4 != 0 {
        return Err(-14); // -EFAULT: невалидный или невыровненный пользовательский адрес
    }

    // 1. Атомарно считываем текущее значение по адресу
    let val_now = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if val_now != val {
        return Err(-11); // -EAGAIN: значение изменилось до засыпания, немедленный возврат
    }

    // 2. Рассчитываем абсолютный дедлайн таймаута
    let deadline_ticks = if timeout_ptr != 0 {
        if timeout_ptr >= USER_SPACE_END {
            return Err(-14);
        }
        let ts = unsafe { core::ptr::read_unaligned(timeout_ptr as *const Timespec) };
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return Err(-22); // -EINVAL
        }
        // 100 Гц PIT: 1 тик = 10 миллисекунд
        let ticks_from_sec = (ts.tv_sec as u64).saturating_mul(100);
        let ticks_from_nsec = (ts.tv_nsec as u64 + 9_999_999) / 10_000_000;
        let delta = ticks_from_sec.saturating_add(ticks_from_nsec);
        Some(crate::arch::x86_64::timer::ticks().saturating_add(delta))
    } else {
        None
    };

    let cur_id = scheduler::current_thread_id();

    // 3. Ставим поток в очередь ожидания
    {
        let mut queues = FUTEX_QUEUES.lock();
        queues
            .entry(uaddr)
            .or_insert_with(VecDeque::new)
            .push_back(FutexWaitNode {
                thread_id: cur_id,
                deadline_ticks,
            });
    }

    // 4. Переводим поток в состояние Blocked и отдаем процессор другому потоку
    scheduler::block_current(deadline_ticks);

    // 5. После возврата проверяем, не наступил ли таймаут
    if let Some(deadline) = deadline_ticks {
        if crate::arch::x86_64::timer::ticks() >= deadline {
            let mut queues = FUTEX_QUEUES.lock();
            if let Some(q) = queues.get_mut(&uaddr) {
                q.retain(|node| node.thread_id != cur_id);
                if q.is_empty() {
                    queues.remove(&uaddr);
                }
            }
            return Err(-110); // -ETIMEDOUT
        }
    }

    Ok(0)
}

/// Пробуждает до `count` потоков, ожидающих на футексе `uaddr`.
/// Возвращает число реально пробужденных потоков.
pub fn futex_wake(uaddr: u64, count: u32) -> Result<u64, i64> {
    if uaddr >= USER_SPACE_END {
        return Err(-14);
    }

    let mut woken = 0u64;
    let mut queues = FUTEX_QUEUES.lock();
    if let Some(q) = queues.get_mut(&uaddr) {
        while woken < (count as u64) {
            if let Some(node) = q.pop_front() {
                scheduler::unblock_thread(node.thread_id);
                woken += 1;
            } else {
                break;
            }
        }
        if q.is_empty() {
            queues.remove(&uaddr);
        }
    }

    Ok(woken)
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

    match cmd {
        FUTEX_WAIT => match futex_wait(uaddr, val as u32, timeout_ptr) {
            Ok(res) => res,
            Err(err) => (-(err.abs())) as u64,
        },
        FUTEX_WAKE => match futex_wake(uaddr, val as u32) {
            Ok(res) => res,
            Err(err) => (-(err.abs())) as u64,
        },
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // Requeue: будит до `val` потоков на `uaddr`, а оставшиеся до `val3` перемещает на `uaddr2`
            if uaddr >= USER_SPACE_END || uaddr2 >= USER_SPACE_END {
                return (-(14i64)) as u64;
            }
            let mut woken = 0u64;
            let mut queues = FUTEX_QUEUES.lock();
            let mut to_move = VecDeque::new();
            if let Some(q) = queues.get_mut(&uaddr) {
                while woken < val {
                    if let Some(node) = q.pop_front() {
                        scheduler::unblock_thread(node.thread_id);
                        woken += 1;
                    } else {
                        break;
                    }
                }
                while to_move.len() < (val3 as usize) {
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
                let target_q = queues.entry(uaddr2).or_insert_with(VecDeque::new);
                target_q.extend(to_move);
            }
            woken
        }
        _ => {
            // Неподдерживаемая операция футекса
            (-(38i64)) as u64 // -ENOSYS
        }
    }
}
