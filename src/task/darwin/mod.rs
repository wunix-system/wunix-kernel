//! Подсистема эмуляции Darwin / macOS (XNU) ядра Wunix.
//!
//! В macOS вызовы ядра разделены на классы через старшие биты регистра RAX:
//! - Класс 0x0100_0000: Mach traps
//! - Класс 0x0200_0000: BSD syscalls
//!
//! Данный модуль изолирован от Linux и Windows: обработка вызовов macOS
//! происходит в собственном пространстве без пересечения номеров с другими ABI.

pub mod macho;

pub use macho::{is_macho, load_macho};

use crate::arch::x86_64::SyscallRegisters;
use crate::serial_println;

pub const SYSCALL_CLASS_MASK: u64 = 0xFF00_0000;
pub const SYSCALL_CLASS_MACH: u64 = 0x0100_0000;
pub const SYSCALL_CLASS_BSD: u64  = 0x0200_0000;

// BSD системные вызовы Darwin
pub const BSD_SYS_EXIT: u64      = 1;
pub const BSD_SYS_FORK: u64      = 2;
pub const BSD_SYS_READ: u64      = 3;
pub const BSD_SYS_WRITE: u64     = 4;
pub const BSD_SYS_OPEN: u64      = 5;
pub const BSD_SYS_CLOSE: u64     = 6;
pub const BSD_SYS_UNLINK: u64    = 10;
pub const BSD_SYS_CHDIR: u64     = 12;
pub const BSD_SYS_GETPID: u64    = 20;
pub const BSD_SYS_GETUID: u64    = 24;
pub const BSD_SYS_GETEUID: u64   = 25;
pub const BSD_SYS_GETEGID: u64   = 43;
pub const BSD_SYS_GETGID: u64    = 47;
pub const BSD_SYS_IOCTL: u64     = 54;
pub const BSD_SYS_EXECVE: u64    = 59;
pub const BSD_SYS_MUNMAP: u64    = 73;
pub const BSD_SYS_MPROTECT: u64  = 74;
pub const BSD_SYS_FSYNC: u64     = 95;
pub const BSD_SYS_SOCKET: u64    = 97;
pub const BSD_SYS_CONNECT: u64   = 98;
pub const BSD_SYS_ACCEPT: u64    = 30;
pub const BSD_SYS_SENDTO: u64    = 133;
pub const BSD_SYS_RECVFROM: u64  = 29;
pub const BSD_SYS_MMAP: u64      = 197;
pub const BSD_SYS_LSEEK: u64     = 199;
pub const BSD_SYS_NANOSLEEP: u64 = 398;

// Ошибки BSD
pub const BSD_ENOSYS: u64 = 78;

/// Диспетчер системных вызовов подсистемы Darwin / macOS
pub fn darwin_syscall_dispatcher(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    regs: &SyscallRegisters,
) -> u64 {
    let class = nr & SYSCALL_CLASS_MASK;
    let call_nr = nr & !SYSCALL_CLASS_MASK;

    if class == SYSCALL_CLASS_BSD || class == 0 {
        match call_nr {
            BSD_SYS_EXIT => {
                crate::task::syscall::sys_exit(arg1);
            }
            BSD_SYS_READ => {
                crate::task::syscall::sys_read(arg1, arg2, arg3)
            }
            BSD_SYS_WRITE => {
                crate::task::syscall::sys_write(arg1, arg2, arg3)
            }
            BSD_SYS_OPEN => {
                crate::task::syscall::sys_open(arg1, arg2, arg3)
            }
            BSD_SYS_CLOSE => {
                crate::task::syscall::sys_close(arg1)
            }
            BSD_SYS_UNLINK => {
                crate::task::syscall::sys_unlink(arg1)
            }
            BSD_SYS_CHDIR => {
                crate::task::syscall::sys_chdir(arg1)
            }
            BSD_SYS_GETPID => {
                crate::task::current_thread_tgid()
            }
            BSD_SYS_GETUID => {
                crate::task::current_thread_uid() as u64
            }
            BSD_SYS_GETEUID => {
                crate::task::current_thread_euid() as u64
            }
            BSD_SYS_GETGID => {
                crate::task::current_thread_gid() as u64
            }
            BSD_SYS_GETEGID => {
                crate::task::current_thread_egid() as u64
            }
            BSD_SYS_EXECVE => {
                crate::task::syscall::sys_execve(arg1, arg2, arg3)
            }
            BSD_SYS_MMAP => {
                crate::task::syscall::sys_mmap(arg1, arg2, arg3, arg4, arg5, regs.r9)
            }
            BSD_SYS_MUNMAP => {
                crate::task::syscall::sys_munmap(arg1, arg2)
            }
            BSD_SYS_MPROTECT => {
                crate::task::syscall::sys_mprotect(arg1, arg2, arg3)
            }
            BSD_SYS_LSEEK => {
                crate::task::syscall::sys_lseek(arg1, arg2 as i64, arg3)
            }
            BSD_SYS_NANOSLEEP => {
                crate::task::syscall::sys_nanosleep(arg1, arg2)
            }
            _ => {
                serial_println!("[darwin-bsd] unhandled BSD syscall nr={}", call_nr);
                (-(BSD_ENOSYS as i64)) as u64
            }
        }
    } else if class == SYSCALL_CLASS_MACH {
        match call_nr as i32 {
            -26 => { // mach_reply_port
                0 // Успешный фиктивный порт
            }
            -27 => { // thread_self_trap
                1 // Порт текущего потока
            }
            -28 => { // task_self_trap
                2 // Порт текущей задачи
            }
            -29 => { // host_self_trap
                3 // Порт хоста
            }
            -31 => { // mach_msg_trap
                serial_println!("[darwin-mach] mach_msg_trap called");
                0 // KERN_SUCCESS
            }
            _ => {
                serial_println!("[darwin-mach] unhandled Mach trap nr={}", call_nr as i32);
                1 // KERN_FAILURE
            }
        }
    } else {
        serial_println!("[darwin] unknown syscall class {:#x}, nr={:#x}", class, nr);
        (-(BSD_ENOSYS as i64)) as u64
    }
}
