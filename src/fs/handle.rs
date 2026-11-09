use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use crate::drivers::keyboard::RingBuffer;
use crate::fs::socket::SockAddrIn;
use crate::fs::vfs::Inode;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_NONBLOCK: u32 = 0x800;
pub const O_CLOEXEC: u32 = 0x80000;

pub const EPOLLIN: u32 = 0x0001;
pub const EPOLLPRI: u32 = 0x0002;
pub const EPOLLOUT: u32 = 0x0004;
pub const EPOLLERR: u32 = 0x0008;
pub const EPOLLHUP: u32 = 0x0010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLET: u32 = 1 << 31;

/// Трейт абстрактного дескриптора открытого файла / сокета / пайпа / epoll (Linux VFS handle)
pub trait FileHandle: Send + Sync {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64>;
    fn write(&self, buf: &[u8]) -> Result<usize, i64>;
    fn poll_events(&self) -> u32 {
        0
    }
    fn is_tty(&self) -> bool {
        false
    }
    fn get_flags(&self) -> u32 {
        0
    }
    fn set_flags(&self, _flags: u32) {}
    fn close(&self) {}
    fn as_inode(&self) -> Option<Arc<dyn Inode>> {
        None
    }
    fn offset(&self) -> usize {
        0
    }
    fn set_offset(&self, _offset: usize) {}
    fn epoll_ctl(&self, _op: u32, _fd: u64, _event_ptr: u64) -> Result<u64, i64> {
        Err(-22)
    }
    fn epoll_wait(&self, _events_ptr: u64, _maxevents: u32, _timeout_ms: i32) -> Result<u64, i64> {
        Err(-22)
    }
    fn socket_connect(&self, _addr: &SockAddrIn) -> Result<(), i64> {
        Err(-88) // -ENOTSOCK
    }
    fn socket_bind(&self, _addr: &SockAddrIn) -> Result<(), i64> {
        Err(-88)
    }
    fn socket_listen(&self, _backlog: i32) -> Result<(), i64> {
        Err(-88)
    }
    fn socket_accept(&self, _flags: u32) -> Result<(u64, SockAddrIn), i64> {
        Err(-88)
    }
    fn socket_sendto(&self, _buf: &[u8], _flags: i32, _dest: Option<&SockAddrIn>) -> Result<usize, i64> {
        Err(-88)
    }
    fn socket_recvfrom(&self, _buf: &mut [u8], _flags: i32) -> Result<(usize, SockAddrIn), i64> {
        Err(-88)
    }
    fn socket_getsockname(&self) -> Result<SockAddrIn, i64> {
        Err(-88)
    }
    fn socket_getpeername(&self) -> Result<SockAddrIn, i64> {
        Err(-88)
    }
    fn socket_shutdown(&self, _how: i32) -> Result<(), i64> {
        Err(-88)
    }
    fn socket_setsockopt(&self, _level: i32, _optname: i32, _optval: &[u8]) -> Result<(), i64> {
        Err(-88)
    }
    fn socket_getsockopt(&self, _level: i32, _optname: i32) -> Result<alloc::vec::Vec<u8>, i64> {
        Err(-88)
    }
}

pub struct RegularFileHandle {
    pub inode: Arc<dyn Inode>,
    pub offset: Mutex<usize>,
    pub flags: AtomicU32,
}

impl FileHandle for RegularFileHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let mut offset_guard = self.offset.lock();
        let bytes_read = self.inode.read_at(*offset_guard, buf).map_err(|_| -9i64)?;
        *offset_guard += bytes_read;
        Ok(bytes_read)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        let mut offset_guard = self.offset.lock();
        if (self.flags.load(Ordering::Relaxed) & 0x400) != 0 {
            *offset_guard = self.inode.metadata().size;
        }
        let bytes_written = self.inode.write_at(*offset_guard, buf).map_err(|_| -9i64)?;
        *offset_guard += bytes_written;
        Ok(bytes_written)
    }

    fn poll_events(&self) -> u32 {
        EPOLLIN | EPOLLOUT
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn as_inode(&self) -> Option<Arc<dyn Inode>> {
        Some(self.inode.clone())
    }

    fn offset(&self) -> usize {
        *self.offset.lock()
    }

    fn set_offset(&self, offset: usize) {
        *self.offset.lock() = offset;
    }
}

static STDIN_BUFFER: Mutex<RingBuffer<u8, 512>> = Mutex::new(RingBuffer::new(0));

/// Добавляет байт в буфер стандартного ввода stdin ядра.
/// Защита через without_interrupts предотвращает взаимные блокировки при вызове из обработчиков прерываний.
pub fn push_stdin_byte(b: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut stdin = STDIN_BUFFER.lock();
        let _ = stdin.push(b);
    });
}

/// Полностью очищает буфер ввода stdin (аналог tcflush(TCIFLUSH)).
pub fn flush_stdin() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        STDIN_BUFFER.lock().clear();
    });
}

/// Возвращает текущее количество накопленных байт в буфере stdin (для FIONREAD).
pub fn stdin_buffer_len() -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        STDIN_BUFFER.lock().len()
    })
}

/// Опрашивает аппаратную очередь клавиатуры и COM1 serial, перенося символы в буфер stdin.
/// Возвращает true, если хотя бы один байт был принят.
pub fn pump_stdin() -> bool {
    let mut got = false;
    let icrnl = crate::task::syscall::fs::is_icrnl_enabled();

    while let Some(c) = crate::drivers::keyboard::pop_char() {
        if c == '\r' && icrnl {
            push_stdin_byte(b'\n');
        } else {
            let mut byte_buf = [0u8; 4];
            let enc = c.encode_utf8(&mut byte_buf);
            for &b in enc.as_bytes() {
                push_stdin_byte(b);
            }
        }
        got = true;
    }

    let mut rx_count = 0;
    while let Some(b) = crate::drivers::serial::try_receive() {
        if b == 0x03 && crate::task::syscall::fs::is_isig_enabled() {
            crate::task::signal::raise_sigint();
        }
        let byte_to_push = if b == b'\r' && icrnl {
            b'\n'
        } else {
            b
        };
        push_stdin_byte(byte_to_push);
        got = true;
        rx_count += 1;
        if rx_count >= 64 {
            break;
        }
    }

    if got && crate::task::signal::is_sigint_pending_for_current() {
        crate::task::signal::clear_sigint();
        flush_stdin();
        crate::drivers::keyboard::clear_key_queue();
        crate::serial_println!("^C");
        crate::println!("^C");
        crate::task::syscall::proc::sys_exit(130);
    }

    got
}

pub struct StdinHandle;

impl FileHandle for StdinHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            // 1. Опустошаем накопленные байты
            let mut count = 0usize;
            x86_64::instructions::interrupts::without_interrupts(|| {
                let mut stdin = STDIN_BUFFER.lock();
                while count < buf.len() {
                    if let Some(b) = stdin.pop() {
                        if b == 0x03 {
                            crate::task::signal::clear_sigint();
                        }
                        if b == 0x04 && count == 0 {
                            return; // EOF (Ctrl+D)
                        }
                        buf[count] = b;
                        count += 1;
                    } else {
                        break;
                    }
                }
            });

            if count > 0 {
                return Ok(count);
            }

            // 2. Опрашиваем клавиатуру и COM1 через унифицированный насос pump_stdin
            if pump_stdin() {
                continue;
            }

            // Буфер пуст: ждем прерывания (PIT/IRQ1)
            x86_64::instructions::interrupts::enable();
            x86_64::instructions::hlt();
        }
    }

    fn write(&self, _buf: &[u8]) -> Result<usize, i64> {
        Err(-9)
    }

    fn poll_events(&self) -> u32 {
        let _ = pump_stdin();
        x86_64::instructions::interrupts::without_interrupts(|| {
            let stdin = STDIN_BUFFER.lock();
            if !stdin.is_empty() {
                EPOLLIN
            } else {
                0
            }
        })
    }

    fn is_tty(&self) -> bool {
        true
    }
}

pub struct StdoutStderrHandle;

impl FileHandle for StdoutStderrHandle {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, i64> {
        Err(-9)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        let mut start = 0;
        while start < buf.len() {
            match core::str::from_utf8(&buf[start..]) {
                Ok(valid) => {
                    crate::serial_print!("{}", valid);
                    crate::print!("{}", valid);
                    break;
                }
                Err(err) => {
                    let valid_len = err.valid_up_to();
                    if valid_len > 0 {
                        if let Ok(valid) = core::str::from_utf8(&buf[start..start + valid_len]) {
                            crate::serial_print!("{}", valid);
                            crate::print!("{}", valid);
                        }
                        start += valid_len;
                    }
                    if let Some(error_len) = err.error_len() {
                        for &b in &buf[start..start + error_len] {
                            crate::serial_print!("{}", b as char);
                            crate::print!("{}", b as char);
                        }
                        start += error_len;
                    } else {
                        for &b in &buf[start..] {
                            crate::serial_print!("{}", b as char);
                            crate::print!("{}", b as char);
                        }
                        break;
                    }
                }
            }
        }
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        EPOLLOUT
    }

    fn is_tty(&self) -> bool {
        true
    }
}

pub struct TtyHandle {
    pub flags: AtomicU32,
}

impl TtyHandle {
    pub fn new(flags: u32) -> Self {
        Self {
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for TtyHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        StdinHandle.read(buf)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        StdoutStderrHandle.write(buf)
    }

    fn poll_events(&self) -> u32 {
        StdinHandle.poll_events() | StdoutStderrHandle.poll_events()
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn is_tty(&self) -> bool {
        true
    }
}

pub struct NullHandle {
    pub flags: AtomicU32,
}

impl NullHandle {
    pub fn new(flags: u32) -> Self {
        Self {
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for NullHandle {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, i64> {
        Ok(0)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        EPOLLIN | EPOLLOUT
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }
}

pub struct ZeroHandle {
    pub flags: AtomicU32,
}

impl ZeroHandle {
    pub fn new(flags: u32) -> Self {
        Self {
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for ZeroHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        EPOLLIN | EPOLLOUT
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }
}

pub struct UrandomHandle {
    pub flags: AtomicU32,
}

impl UrandomHandle {
    pub fn new(flags: u32) -> Self {
        Self {
            flags: AtomicU32::new(flags),
        }
    }
}

impl FileHandle for UrandomHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        crate::task::syscall::mem::fill_random_bytes(buf);
        Ok(buf.len())
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        EPOLLIN | EPOLLOUT
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }
}
