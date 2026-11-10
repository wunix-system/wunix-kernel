pub mod at;
pub mod creds;
pub mod dir;
pub mod extended;
pub mod fs;
pub mod ipc;
pub mod mem;
pub mod net;
pub mod poll;
pub mod proc;
pub mod sched;
pub mod signal;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::x86_64::SyscallRegisters;
use crate::fs::insert_file_handle;
use crate::serial_println;

pub use at::*;
pub use creds::*;
pub use dir::*;
pub use extended::*;
pub use fs::*;
pub use ipc::*;
pub use mem::*;
pub use net::*;
pub use poll::*;
pub use proc::*;
pub use sched::*;
pub use signal::*;

pub const E2BIG: u64 = 7;
pub const ENOTSOCK: u64 = 88;
pub const ERANGE: u64 = 34;


// Номера системных вызовов Linux x86_64 ABI
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_YIELD: u64 = 24;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MINCORE: u64 = 27;
pub const SYS_MADVISE: u64 = 28;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_PAUSE: u64 = 34;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_ALARM: u64 = 37;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SOCKETPAIR: u64 = 53;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_VFORK: u64 = 58;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_FCHDIR: u64 = 81;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_CHOWN: u64 = 92;
pub const SYS_FCHOWN: u64 = 93;
pub const SYS_UMASK: u64 = 95;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETRLIMIT: u64 = 97;
pub const SYS_GETRUSAGE: u64 = 98;
pub const SYS_SYSINFO: u64 = 99;
pub const SYS_TIMES: u64 = 100;
pub const SYS_GETUID: u64 = 102;
pub const SYS_SYSLOG: u64 = 103;
pub const SYS_GETGID: u64 = 104;
pub const SYS_SETUID: u64 = 105;
pub const SYS_SETGID: u64 = 106;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_SETSID: u64 = 112;
pub const SYS_SETREUID: u64 = 113;
pub const SYS_SETREGID: u64 = 114;
pub const SYS_GETGROUPS: u64 = 115;
pub const SYS_SETGROUPS: u64 = 116;
pub const SYS_SETRESUID: u64 = 117;
pub const SYS_GETRESUID: u64 = 118;
pub const SYS_SETRESGID: u64 = 119;
pub const SYS_GETRESGID: u64 = 120;
pub const SYS_GETPGID: u64 = 121;
pub const SYS_GETSID: u64 = 124;
pub const SYS_CAPGET: u64 = 125;
pub const SYS_CAPSET: u64 = 126;
pub const SYS_SIGALTSTACK: u64 = 131;
pub const SYS_UTIME: u64 = 132;
pub const SYS_STATFS: u64 = 137;
pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141;
pub const SYS_SCHED_SETPARAM: u64 = 142;
pub const SYS_SCHED_GETPARAM: u64 = 143;
pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
pub const SYS_SCHED_GETSCHEDULER: u64 = 145;
pub const SYS_SCHED_GET_PRIORITY_MAX: u64 = 146;
pub const SYS_SCHED_GET_PRIORITY_MIN: u64 = 147;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_SETRLIMIT: u64 = 160;
pub const SYS_CHROOT: u64 = 161;
pub const SYS_SYNC: u64 = 162;
pub const SYS_SETTIMEOFDAY: u64 = 164;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_UMOUNT2: u64 = 166;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_GETTID: u64 = 186;
pub const SYS_TKILL: u64 = 200;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_CLOCK_SETTIME: u64 = 227;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_CLOCK_GETRES: u64 = 229;
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_TGKILL: u64 = 234;
pub const SYS_UTIMES: u64 = 235;
pub const SYS_WAITID: u64 = 247;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_FCHOWNAT: u64 = 260;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_RENAMEAT: u64 = 264;
pub const SYS_SYMLINKAT: u64 = 266;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FCHMODAT: u64 = 268;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_UTIMENSAT: u64 = 280;
pub const SYS_EPOLL_PWAIT: u64 = 281;
pub const SYS_EVENTFD: u64 = 284;
pub const SYS_FALLOCATE: u64 = 285;
pub const SYS_ACCEPT4: u64 = 288;
pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_GET_ROBUST_LIST: u64 = 274;
pub const SYS_RENAMEAT2: u64 = 316;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_MEMFD_CREATE: u64 = 319;
pub const SYS_COPY_FILE_RANGE: u64 = 326;
pub const SYS_STATX: u64 = 332;
pub const SYS_RSEQ: u64 = 334;
pub const SYS_CLOSE_RANGE: u64 = 436;
pub const SYS_FACCESSAT2: u64 = 439;

// Стандартные коды ошибок POSIX
pub const EPERM: u64 = 1;
pub const ENOENT: u64 = 2;
pub const ESRCH: u64 = 3;
pub const EINTR: u64 = 4;
pub const EIO: u64 = 5;
pub const ENOEXEC: u64 = 8;
pub const EBADF: u64 = 9;
pub const ECHILD: u64 = 10;
pub const EAGAIN: u64 = 11;
pub const ENOMEM: u64 = 12;
pub const EACCES: u64 = 13;
pub const EFAULT: u64 = 14;
pub const EEXIST: u64 = 17;
pub const ENODEV: u64 = 19;
pub const ENOTDIR: u64 = 20;
pub const EISDIR: u64 = 21;
pub const EINVAL: u64 = 22;
pub const ENFILE: u64 = 23;
pub const EMFILE: u64 = 24;
pub const ENOTTY: u64 = 25;
pub const ESPIPE: u64 = 29;
pub const EPIPE: u64 = 32;
pub const ENOSYS: u64 = 38;
pub const ENOTEMPTY: u64 = 39;
pub const EOPNOTSUPP: u64 = 95;
pub const EAFNOSUPPORT: u64 = 97;
pub const ETIMEDOUT: u64 = 110;

// Граница канонического пространства пользователя x86_64
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

// Глобальный менеджер виртуальной памяти (VMA)
static VMA_MANAGER: Mutex<crate::memory::VirtualMemoryManager> =
    Mutex::new(crate::memory::VirtualMemoryManager::new());

// Рабочие директории процессов (по TGID), изолирующие фоновые службы от интерактивного шелла
static PROCESS_CWD: spin::Mutex<alloc::collections::BTreeMap<u64, String>> =
    spin::Mutex::new(alloc::collections::BTreeMap::new());

pub fn get_cwd() -> String {
    let tgid = crate::task::current_thread_tgid();
    let map = PROCESS_CWD.lock();
    if let Some(cwd) = map.get(&tgid) {
        cwd.clone()
    } else {
        String::from("/root")
    }
}

pub fn set_cwd(new_path: &str) {
    let tgid = crate::task::current_thread_tgid();
    set_cwd_for_tgid(tgid, new_path);
}

pub fn set_cwd_for_tgid(tgid: u64, new_path: &str) {
    let mut map = PROCESS_CWD.lock();
    map.insert(tgid, String::from(new_path));
}

pub fn clone_process_cwd(source_tgid: u64, target_tgid: u64) {
    let mut map = PROCESS_CWD.lock();
    let cwd = if let Some(src) = map.get(&source_tgid) {
        src.clone()
    } else {
        String::from("/root")
    };
    map.insert(target_tgid, cwd);
}

pub fn remove_process_cwd(tgid: u64) {
    let mut map = PROCESS_CWD.lock();
    map.remove(&tgid);
}

fn resolve_relative_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with('/') || crate::fs::path::is_windows_subsystem(trimmed) {
        crate::fs::path::normalize_path(trimmed)
    } else {
        let cwd = get_cwd();
        let combined = if cwd == "/" {
            alloc::format!("/{}", trimmed)
        } else {
            alloc::format!("{}/{}", cwd, trimmed)
        };
        crate::fs::path::normalize_path(&combined)
    }
}

#[inline]
fn validate_user_buffer(ptr: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    match ptr.checked_add(len) {
        Some(end) => ptr > 0 && end <= USER_SPACE_END,
        None => false,
    }
}

fn read_user_cstr(ptr: u64, max_len: usize) -> Result<String, u64> {
    if ptr == 0 || ptr >= USER_SPACE_END {
        return Err(EFAULT);
    }
    let mut bytes = Vec::new();
    let mut curr = ptr;
    while bytes.len() < max_len {
        if curr >= USER_SPACE_END {
            return Err(EFAULT);
        }
        let b = unsafe { *(curr as *const u8) };
        if b == 0 {
            break;
        }
        bytes.push(b);
        curr += 1;
    }
    if bytes.len() >= max_len {
        return Err(EINVAL);
    }
    core::str::from_utf8(&bytes)
        .map(String::from)
        .map_err(|_| EINVAL)
}


/// Диспетчер системных вызовов нативного мира Linux x86_64.
pub fn linux_syscall_dispatcher(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    r: &SyscallRegisters,
) -> u64 {
    match nr {
        SYS_READ => sys_read(arg1, arg2, arg3),
        SYS_WRITE => sys_write(arg1, arg2, arg3),
        SYS_OPEN => sys_open(arg1, arg2, arg3),
        SYS_CLOSE => sys_close(arg1),
        SYS_STAT => sys_newfstatat(-100, arg1, arg2, 0),
        SYS_FSTAT => sys_fstat(arg1, arg2),
        SYS_LSTAT => sys_newfstatat(-100, arg1, arg2, 0x100),
        SYS_POLL => sys_poll(arg1, arg2, arg3 as i64),
        SYS_LSEEK => sys_lseek(arg1, arg2 as i64, arg3),
        SYS_MMAP => sys_mmap(arg1, arg2, arg3, arg4, arg5, r.r9),
        SYS_MPROTECT => sys_mprotect(arg1, arg2, arg3),
        SYS_MUNMAP => sys_munmap(arg1, arg2),
        SYS_BRK => sys_brk(arg1),
        SYS_RT_SIGACTION => crate::task::signal::sys_rt_sigaction(arg1, arg2, arg3, arg4),
        SYS_RT_SIGPROCMASK => crate::task::signal::sys_rt_sigprocmask(arg1 as i32, arg2, arg3, arg4),
        SYS_RT_SIGRETURN => 0,
        SYS_IOCTL => sys_ioctl(arg1, arg2, arg3),
        SYS_PREAD64 => sys_pread64(arg1, arg2, arg3, arg4),
        SYS_PWRITE64 => sys_pwrite64(arg1, arg2, arg3, arg4),
        SYS_READV => sys_readv(arg1, arg2, arg3),
        SYS_WRITEV => sys_writev(arg1, arg2, arg3),
        SYS_ACCESS => sys_access(arg1, arg2),
        SYS_PIPE => sys_pipe2(arg1, 0),
        SYS_SELECT => sys_select(arg1, arg2, arg3, arg4, arg5),
        SYS_YIELD => sys_yield(),
        SYS_MINCORE => sys_mincore(arg1, arg2, arg3),
        SYS_MADVISE => sys_madvise(arg1, arg2, arg3),
        SYS_DUP => sys_dup(arg1),
        SYS_DUP2 => sys_dup3(arg1, arg2, 0),
        SYS_PAUSE => sys_pause(),
        SYS_NANOSLEEP => sys_nanosleep(arg1, arg2),
        SYS_ALARM => sys_alarm(arg1),
        SYS_GETPID => crate::task::current_thread_tgid(),
        SYS_SOCKET => match crate::fs::socket::create_socket(arg1 as u16, arg2 as u32, arg3 as i32) {
            Ok(sock) => insert_file_handle(sock),
            Err(err) => (-(err.abs())) as u64,
        },
        SYS_CONNECT => sys_connect(arg1, arg2, arg3),
        SYS_BIND => sys_bind(arg1, arg2, arg3),
        SYS_LISTEN => sys_listen(arg1, arg2 as i32),
        SYS_ACCEPT => sys_accept4(arg1, arg2, arg3, 0),
        SYS_ACCEPT4 => sys_accept4(arg1, arg2, arg3, arg4 as u32),
        SYS_SENDTO => sys_sendto(arg1, arg2, arg3, arg4 as i32, arg5, r.r9),
        SYS_RECVFROM => sys_recvfrom(arg1, arg2, arg3, arg4 as i32, arg5, r.r9),
        SYS_SENDMSG => sys_sendmsg(arg1, arg2, arg3),
        SYS_RECVMSG => sys_recvmsg(arg1, arg2, arg3),
        SYS_SHUTDOWN => sys_shutdown(arg1, arg2),
        SYS_GETSOCKNAME => sys_getsockname(arg1, arg2, arg3),
        SYS_GETPEERNAME => sys_getpeername(arg1, arg2, arg3),
        SYS_SOCKETPAIR => sys_socketpair(arg1, arg2, arg3, arg4),
        SYS_SETSOCKOPT => sys_setsockopt(arg1, arg2, arg3, arg4, arg5),
        SYS_GETSOCKOPT => sys_getsockopt(arg1, arg2, arg3, arg4, arg5),
        SYS_MOUNT => sys_mount(arg1, arg2, arg3, arg4, arg5),
        SYS_UMOUNT2 => sys_umount2(arg1, arg2),
        SYS_SYNC => sys_sync(),
        SYS_SETTIMEOFDAY => sys_settimeofday(arg1, arg2),
        SYS_REBOOT => sys_reboot(arg1, arg2, arg3, arg4),
        SYS_CLONE => sys_clone(arg1, arg2, arg3, arg4, arg5, r),
        SYS_FORK => sys_fork(r),
        SYS_VFORK => sys_vfork(r),
        SYS_EXECVE => sys_execve(arg1, arg2, arg3),
        SYS_EXIT => sys_exit(arg1),
        SYS_WAIT4 => sys_wait4(arg1 as i64, arg2, arg3, arg4),
        SYS_KILL => sys_kill(arg1 as i64, arg2),
        SYS_TGKILL => sys_tgkill(arg1 as i64, arg2 as i64, arg3),
        SYS_TKILL => sys_tkill(arg1 as i64, arg2),
        SYS_UNAME => sys_uname(arg1),
        SYS_FCNTL => sys_fcntl(arg1, arg2, arg3),
        SYS_FLOCK => sys_flock(arg1, arg2),
        SYS_FSYNC => sys_fsync(arg1),
        SYS_FDATASYNC => sys_fdatasync(arg1),
        SYS_TRUNCATE => sys_truncate(arg1, arg2),
        SYS_FTRUNCATE => sys_ftruncate(arg1, arg2),
        SYS_GETCWD => sys_getcwd(arg1, arg2),
        SYS_CHDIR => sys_chdir(arg1),
        SYS_FCHDIR => sys_fchdir(arg1),
        SYS_RENAME => sys_rename(arg1, arg2),
        SYS_MKDIR => sys_mkdir(arg1, arg2),
        SYS_RMDIR => sys_rmdir(arg1),
        SYS_UNLINK => sys_unlink(arg1),
        SYS_SYMLINK => sys_symlink(arg1, arg2),
        SYS_READLINK => sys_readlink(arg1, arg2, arg3),
        SYS_CHMOD => sys_chmod(arg1, arg2),
        SYS_FCHMOD => sys_fchmod(arg1, arg2),
        SYS_CHOWN => sys_chown(arg1, arg2, arg3),
        SYS_FCHOWN => sys_fchown(arg1, arg2, arg3),
        SYS_UMASK => sys_umask(arg1),
        SYS_GETTIMEOFDAY => sys_gettimeofday(arg1, arg2),
        SYS_GETRLIMIT => sys_prlimit64(0, arg1, 0, arg2),
        SYS_GETRUSAGE => sys_getrusage(arg1 as i32, arg2),
        SYS_SYSINFO => sys_sysinfo(arg1),
        SYS_TIMES => sys_times(arg1),
        SYS_GETUID => sys_getuid(),
        SYS_GETEUID => sys_geteuid(),
        SYS_GETGID => sys_getgid(),
        SYS_GETEGID => sys_getegid(),
        SYS_SETUID => sys_setuid(arg1),
        SYS_SETGID => sys_setgid(arg1),
        SYS_SETPGID => sys_setpgid(arg1, arg2),
        SYS_GETPPID => sys_getppid(),
        SYS_GETPGRP => sys_getpgrp(),
        SYS_SETREUID => sys_setreuid(arg1, arg2),
        SYS_SETREGID => sys_setregid(arg1, arg2),
        SYS_GETGROUPS => sys_getgroups(arg1, arg2),
        SYS_SETGROUPS => sys_setgroups(arg1, arg2),
        SYS_SETRESUID => sys_setresuid(arg1, arg2, arg3),
        SYS_GETRESUID => sys_getresuid(arg1, arg2, arg3),
        SYS_SETRESGID => sys_setresgid(arg1, arg2, arg3),
        SYS_GETRESGID => sys_getresgid(arg1, arg2, arg3),
        SYS_GETPGID => sys_getpgid(arg1),
        SYS_GETSID => sys_getsid(arg1),
        SYS_CAPGET => sys_capget(arg1, arg2),
        SYS_CAPSET => sys_capset(arg1, arg2),
        SYS_SETSID => sys_setsid(),
        SYS_SIGALTSTACK => crate::task::signal::sys_sigaltstack(arg1, arg2),
        SYS_UTIME => sys_utimensat(-100, arg1, arg2, 0),
        SYS_STATFS => sys_statfs(arg1, arg2),
        SYS_GETPRIORITY => sys_getpriority(arg1, arg2),
        SYS_SETPRIORITY => sys_setpriority(arg1, arg2, arg3),
        SYS_SCHED_SETPARAM => sys_sched_setparam(arg1, arg2),
        SYS_SCHED_GETPARAM => sys_sched_getparam(arg1, arg2),
        SYS_SCHED_SETSCHEDULER => sys_sched_setscheduler(arg1, arg2, arg3),
        SYS_SCHED_GETSCHEDULER => sys_sched_getscheduler(arg1),
        SYS_SCHED_GET_PRIORITY_MAX => sys_sched_get_priority_max(arg1),
        SYS_SCHED_GET_PRIORITY_MIN => sys_sched_get_priority_min(arg1),
        SYS_PRCTL => sys_prctl(arg1, arg2, arg3, arg4, arg5),
        SYS_ARCH_PRCTL => sys_arch_prctl(arg1, arg2),
        SYS_SETRLIMIT => sys_prlimit64(0, arg1, arg2, 0),
        SYS_CHROOT => sys_chroot(arg1),
        SYS_GETTID => crate::task::current_thread_id().0,
        SYS_FUTEX => crate::task::futex::sys_futex(arg1, arg2, arg3, arg4, arg5, r.r9),
        SYS_SCHED_GETAFFINITY => sys_sched_getaffinity(arg1, arg2, arg3),
        SYS_EPOLL_CREATE => sys_epoll_create1(0),
        SYS_GETDENTS64 => sys_getdents64(arg1, arg2, arg3),
        SYS_SET_TID_ADDRESS => {
            crate::task::set_current_clear_child_tid(arg1);
            crate::task::current_thread_id().0
        }
        SYS_CLOCK_SETTIME => sys_clock_settime(arg1, arg2),
        SYS_CLOCK_GETTIME => sys_clock_gettime(arg1, arg2),
        SYS_CLOCK_GETRES => sys_clock_getres(arg1, arg2),
        SYS_CLOCK_NANOSLEEP => sys_clock_nanosleep(arg1, arg2, arg3, arg4),
        SYS_EXIT_GROUP => sys_exit_group(arg1),
        SYS_EPOLL_WAIT => sys_epoll_wait(arg1, arg2, arg3, arg4 as i64),
        SYS_EPOLL_CTL => sys_epoll_ctl(arg1, arg2, arg3, arg4),
        SYS_UTIMES => sys_utimes(arg1, arg2),
        SYS_WAITID => sys_waitid(arg1, arg2, arg3, arg4, arg5),
        SYS_OPENAT => sys_openat(arg1 as i64, arg2, arg3, arg4),
        SYS_MKDIRAT => sys_mkdirat(arg1 as i64, arg2, arg3),
        SYS_FCHOWNAT => sys_fchownat(arg1 as i64, arg2, arg3, arg4, arg5),
        SYS_NEWFSTATAT => sys_newfstatat(arg1 as i64, arg2, arg3, arg4),
        SYS_UNLINKAT => sys_unlinkat(arg1 as i64, arg2, arg3),
        SYS_RENAMEAT => sys_renameat(arg1 as i64, arg2, arg3 as i64, arg4),
        SYS_SYMLINKAT => sys_symlinkat(arg1, arg2 as i64, arg3),
        SYS_READLINKAT => sys_readlinkat(arg1 as i64, arg2, arg3, arg4),
        SYS_FCHMODAT => sys_fchmodat(arg1 as i64, arg2, arg3, arg4),
        SYS_FACCESSAT => sys_faccessat(arg1 as i64, arg2, arg3, 0),
        SYS_PSELECT6 => sys_pselect6(arg1, arg2, arg3, arg4, arg5, r.r9),
        SYS_PPOLL => sys_ppoll(arg1, arg2, arg3, arg4, arg5),
        SYS_UTIMENSAT => sys_utimensat(arg1 as i64, arg2, arg3, arg4),
        SYS_EPOLL_PWAIT => sys_epoll_wait(arg1, arg2, arg3, arg4 as i64),
        SYS_EVENTFD => sys_eventfd2(arg1 as u32, 0),
        SYS_FALLOCATE => sys_fallocate(arg1, arg2, arg3, arg4),
        SYS_EVENTFD2 => sys_eventfd2(arg1 as u32, arg2 as u32),
        SYS_EPOLL_CREATE1 => sys_epoll_create1(arg1),
        SYS_DUP3 => sys_dup3(arg1, arg2, arg3),
        SYS_PIPE2 => sys_pipe2(arg1, arg2),
        SYS_PRLIMIT64 => sys_prlimit64(arg1, arg2, arg3, arg4),
        SYS_RENAMEAT2 => sys_renameat2(arg1 as i64, arg2, arg3 as i64, arg4, arg5),
        SYS_GETRANDOM => sys_getrandom(arg1, arg2, arg3),
        SYS_MEMFD_CREATE => sys_memfd_create(arg1, arg2),
        SYS_COPY_FILE_RANGE => sys_copy_file_range(arg1, arg2, arg3, arg4, arg5, r.r9),
        SYS_STATX => sys_statx(arg1 as i64, arg2, arg3, arg4, arg5),
        SYS_SET_ROBUST_LIST => (-(ENOSYS as i64)) as u64,
        SYS_GET_ROBUST_LIST => (-(ENOSYS as i64)) as u64,
        SYS_RSEQ => (-(ENOSYS as i64)) as u64, // Fallback to standard TLS
        SYS_CLOSE_RANGE => sys_close_range(arg1, arg2, arg3),
        SYS_FACCESSAT2 => sys_faccessat2(arg1 as i64, arg2, arg3, arg4),
        _ => {
            serial_println!("[syscall-linux] unhandled syscall nr={}", nr);
            (-(ENOSYS as i64)) as u64
        }
    }
}

/// Главный межсистемный роутер системных вызовов ядра Wunix.
/// Вызывается напрямую из ассемблерного трамплина `syscall_entry` и маршрутизирует
/// вызов в изолированный мир (ABI Personality) текущего процесса.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatcher(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    regs: *mut SyscallRegisters,
) -> u64 {
    // Прерываем выполнение дочернего процесса при получении сигнала SIGINT
    if crate::task::signal::is_sigint_pending_for_current() {
        crate::task::signal::clear_sigint();
        crate::fs::handle::flush_stdin();
        crate::drivers::keyboard::clear_key_queue();
        crate::serial_println!("^C");
        crate::println!("^C");
        sys_exit(130);
    }

    let r = unsafe { &*regs };
    let personality = crate::task::current_thread_abi();

    let res = match personality {
        crate::task::abi::AbiPersonality::Linux => {
            linux_syscall_dispatcher(nr, arg1, arg2, arg3, arg4, arg5, r)
        }
        crate::task::abi::AbiPersonality::WindowsNt => {
            crate::task::win_syscall::win_syscall_dispatcher(nr, arg1, arg2, arg3, arg4, arg5, r)
        }
        crate::task::abi::AbiPersonality::Darwin => {
            crate::task::darwin::darwin_syscall_dispatcher(nr, arg1, arg2, arg3, arg4, arg5, r)
        }
    };

    if crate::task::signal::is_sigint_pending_for_current() {
        crate::task::signal::clear_sigint();
        crate::fs::handle::flush_stdin();
        crate::drivers::keyboard::clear_key_queue();
        crate::serial_println!("^C");
        crate::println!("^C");
        sys_exit(130);
    }

    res
}

pub fn alloc_kernel_stack() -> (alloc::boxed::Box<[u8]>, u64) {
    let mut stack = alloc::vec![0u8; crate::task::thread::STACK_SIZE].into_boxed_slice();
    let stack_end = stack.as_mut_ptr() as usize + crate::task::thread::STACK_SIZE;
    (stack, stack_end as u64)
}
