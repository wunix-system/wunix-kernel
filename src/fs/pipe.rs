use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use super::{FileHandle, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, O_NONBLOCK};
use crate::task::scheduler;
use crate::task::thread::ThreadId;

const PIPE_CAPACITY: usize = 65536; // 64 КиБ кольцевой буфер канала

pub struct PipeBuffer {
    data: alloc::boxed::Box<[u8]>,
    head: usize,
    tail: usize,
    len: usize,
    readers_open: usize,
    writers_open: usize,
    waiting_readers: Vec<ThreadId>,
    waiting_writers: Vec<ThreadId>,
}

impl PipeBuffer {
    pub fn new() -> Self {
        Self {
            data: alloc::vec![0u8; PIPE_CAPACITY].into_boxed_slice(),
            head: 0,
            tail: 0,
            len: 0,
            readers_open: 1,
            writers_open: 1,
            waiting_readers: Vec::new(),
            waiting_writers: Vec::new(),
        }
    }
}

pub struct PipeReadHandle {
    pipe: Arc<Mutex<PipeBuffer>>,
    flags: AtomicU32,
}

impl PipeReadHandle {
    pub fn new(pipe: Arc<Mutex<PipeBuffer>>, flags: u32) -> Self {
        Self {
            pipe,
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for PipeReadHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut pipe = self.pipe.lock();

            if pipe.len > 0 {
                let to_read = buf.len().min(pipe.len);
                for i in 0..to_read {
                    buf[i] = pipe.data[(pipe.head + i) % PIPE_CAPACITY];
                }
                pipe.head = (pipe.head + to_read) % PIPE_CAPACITY;
                pipe.len -= to_read;

                // Пробуждаем ожидающих писателей
                let writers = core::mem::take(&mut pipe.waiting_writers);
                drop(pipe);
                for tid in writers {
                    scheduler::unblock_thread(tid);
                }

                return Ok(to_read);
            }

            // Буфер пуст: если писатели закрыли дескриптор, возвращаем EOF (0 байт)
            if pipe.writers_open == 0 {
                return Ok(0);
            }

            // Если неблокирующий режим (O_NONBLOCK), возвращаем -EAGAIN для рантайма Go Netpoller
            if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                return Err(-11); // -EAGAIN
            }

            // Блокирующий режим: засыпаем до появления данных в пайпе
            let cur_id = scheduler::current_thread_id();
            pipe.waiting_readers.push(cur_id);
            drop(pipe);

            scheduler::block_current(None);
        }
    }

    fn write(&self, _buf: &[u8]) -> Result<usize, i64> {
        Err(-9) // -EBADF: нельзя писать в конец для чтения
    }

    fn poll_events(&self) -> u32 {
        let pipe = self.pipe.lock();
        let mut ev = 0;
        if pipe.len > 0 {
            ev |= EPOLLIN;
        }
        if pipe.writers_open == 0 {
            ev |= EPOLLHUP | EPOLLIN;
        }
        ev
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn close(&self) {
        let mut pipe = self.pipe.lock();
        if pipe.readers_open > 0 {
            pipe.readers_open -= 1;
        }
        let writers = core::mem::take(&mut pipe.waiting_writers);
        drop(pipe);
        for tid in writers {
            scheduler::unblock_thread(tid);
        }
    }
}

pub struct PipeWriteHandle {
    pipe: Arc<Mutex<PipeBuffer>>,
    flags: AtomicU32,
}

impl PipeWriteHandle {
    pub fn new(pipe: Arc<Mutex<PipeBuffer>>, flags: u32) -> Self {
        Self {
            pipe,
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for PipeWriteHandle {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, i64> {
        Err(-9) // -EBADF: нельзя читать из конца для записи
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut pipe = self.pipe.lock();

            // Если читатели закрыли дескриптор, бросаем -EPIPE
            if pipe.readers_open == 0 {
                return Err(-32); // -EPIPE
            }

            let available = PIPE_CAPACITY - pipe.len;
            if available > 0 {
                let to_write = buf.len().min(available);
                let tail = pipe.tail;
                for i in 0..to_write {
                    pipe.data[(tail + i) % PIPE_CAPACITY] = buf[i];
                }
                pipe.tail = (tail + to_write) % PIPE_CAPACITY;
                pipe.len += to_write;

                // Пробуждаем ожидающих читателей
                let readers = core::mem::take(&mut pipe.waiting_readers);
                drop(pipe);
                for tid in readers {
                    scheduler::unblock_thread(tid);
                }

                return Ok(to_write);
            }

            // Буфер полон: в неблокирующем режиме возвращаем -EAGAIN
            if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                return Err(-11); // -EAGAIN
            }

            // Блокирующий режим: ждем освобождения места
            let cur_id = scheduler::current_thread_id();
            pipe.waiting_writers.push(cur_id);
            drop(pipe);

            scheduler::block_current(None);
        }
    }

    fn poll_events(&self) -> u32 {
        let pipe = self.pipe.lock();
        let mut ev = 0;
        if pipe.len < PIPE_CAPACITY {
            ev |= EPOLLOUT;
        }
        if pipe.readers_open == 0 {
            ev |= EPOLLERR;
        }
        ev
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn close(&self) {
        let mut pipe = self.pipe.lock();
        if pipe.writers_open > 0 {
            pipe.writers_open -= 1;
        }
        let readers = core::mem::take(&mut pipe.waiting_readers);
        drop(pipe);
        for tid in readers {
            scheduler::unblock_thread(tid);
        }
    }
}

pub struct SocketPairHandle {
    rx: Arc<Mutex<PipeBuffer>>,
    tx: Arc<Mutex<PipeBuffer>>,
    flags: AtomicU32,
}

impl SocketPairHandle {
    pub fn new(rx: Arc<Mutex<PipeBuffer>>, tx: Arc<Mutex<PipeBuffer>>, flags: u32) -> Self {
        Self {
            rx,
            tx,
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for SocketPairHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut pipe = self.rx.lock();
            if pipe.len > 0 {
                let to_read = buf.len().min(pipe.len);
                for i in 0..to_read {
                    let head = pipe.head;
                    buf[i] = pipe.data[head];
                    pipe.head = (head + 1) % PIPE_CAPACITY;
                }
                pipe.len -= to_read;

                let writers = core::mem::take(&mut pipe.waiting_writers);
                drop(pipe);
                for tid in writers {
                    scheduler::unblock_thread(tid);
                }

                return Ok(to_read);
            }

            if pipe.writers_open == 0 {
                return Ok(0);
            }

            if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                return Err(-11); // -EAGAIN
            }

            let cur_id = scheduler::current_thread_id();
            pipe.waiting_readers.push(cur_id);
            drop(pipe);

            scheduler::block_current(None);
        }
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut pipe = self.tx.lock();
            if pipe.readers_open == 0 {
                return Err(-32); // -EPIPE
            }

            let available = PIPE_CAPACITY - pipe.len;
            if available > 0 {
                let to_write = buf.len().min(available);
                for i in 0..to_write {
                    let tail = pipe.tail;
                    pipe.data[tail] = buf[i];
                    pipe.tail = (tail + 1) % PIPE_CAPACITY;
                }
                pipe.len += to_write;

                let readers = core::mem::take(&mut pipe.waiting_readers);
                drop(pipe);
                for tid in readers {
                    scheduler::unblock_thread(tid);
                }

                return Ok(to_write);
            }

            if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                return Err(-11); // -EAGAIN
            }

            let cur_id = scheduler::current_thread_id();
            pipe.waiting_writers.push(cur_id);
            drop(pipe);

            scheduler::block_current(None);
        }
    }

    fn poll_events(&self) -> u32 {
        let mut ev = 0;
        {
            let rx = self.rx.lock();
            if rx.len > 0 {
                ev |= EPOLLIN;
            }
            if rx.writers_open == 0 {
                ev |= EPOLLHUP;
            }
        }
        {
            let tx = self.tx.lock();
            if tx.len < PIPE_CAPACITY {
                ev |= EPOLLOUT;
            }
            if tx.readers_open == 0 {
                ev |= EPOLLERR;
            }
        }
        ev
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn close(&self) {
        {
            let mut rx = self.rx.lock();
            if rx.readers_open > 0 {
                rx.readers_open -= 1;
            }
            let writers = core::mem::take(&mut rx.waiting_writers);
            drop(rx);
            for tid in writers {
                scheduler::unblock_thread(tid);
            }
        }
        {
            let mut tx = self.tx.lock();
            if tx.writers_open > 0 {
                tx.writers_open -= 1;
            }
            let readers = core::mem::take(&mut tx.waiting_readers);
            drop(tx);
            for tid in readers {
                scheduler::unblock_thread(tid);
            }
        }
    }
}

pub fn create_socketpair(flags: u32) -> (Arc<dyn FileHandle>, Arc<dyn FileHandle>) {
    let a_to_b = Arc::new(Mutex::new(PipeBuffer::new()));
    let b_to_a = Arc::new(Mutex::new(PipeBuffer::new()));

    let sock_a = Arc::new(SocketPairHandle::new(b_to_a.clone(), a_to_b.clone(), flags));
    let sock_b = Arc::new(SocketPairHandle::new(a_to_b, b_to_a, flags));

    (sock_a, sock_b)
}
