use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use crate::arch::x86_64::SyscallRegisters;
use crate::serial_println;
use super::{
    get_cwd, read_user_cstr, resolve_relative_path, set_cwd, validate_user_buffer, EACCES, EFAULT,
    EINTR, EINVAL, ENOENT, ENOEXEC, ENOMEM, ENOTDIR, EPERM, ERANGE,
};

// Битовая маска интерактивных процессов/оболочек на каждом уровне child_stack_depth (0..31).
// Бит 0 инициализирован в 1 (первичный системный процесс PID 1: /bin/init или /bin/wsh).
static CHILD_SHELL_MASK: AtomicU32 = AtomicU32::new(1);

pub fn set_child_is_shell(depth: usize, is_shell: bool) {
    if depth < 32 {
        if is_shell {
            CHILD_SHELL_MASK.fetch_or(1 << depth, Ordering::SeqCst);
        } else {
            CHILD_SHELL_MASK.fetch_and(!(1 << depth), Ordering::SeqCst);
        }
    }
}

pub fn is_current_interactive_shell() -> bool {
    let depth = crate::arch::x86_64::syscall::current_child_stack_depth();
    if depth >= 32 {
        return false;
    }
    (CHILD_SHELL_MASK.load(Ordering::Relaxed) & (1 << depth)) != 0
}

pub fn is_shell_binary_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "wsh" || name == "sh" || name == "login" || name == "init"
}

static NEXT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(10);

pub fn sys_clone(
    flags: u64,
    newsp: u64,
    parent_tidptr: u64,
    child_tidptr: u64,
    tls: u64,
    regs: &SyscallRegisters,
) -> u64 {
    let child_tid = crate::task::ThreadId::new();
    let (stack, kernel_stack_top) = crate::task::syscall::alloc_kernel_stack();

    let child_user_rsp = if newsp != 0 { newsp } else { regs.user_rsp };
    let child_user_rip = regs.user_rip;

    let child_fs_base = if (flags & 0x00080000) != 0 || tls != 0 {
        tls
    } else {
        crate::task::get_current_fs_base()
    };

    // Формируем начальный стек-фрейм (96 байт) для context_switch и ret_from_fork:
    // [0]: r15
    // [1]: r14
    // [2]: r13 (mp в рантайме Go)
    // [3]: r12 (fn в рантайме Go)
    // [4]: rbx
    // [5]: rbp
    // [6]: ret_from_fork
    // [7]: r9  (gp в рантайме Go)
    // [8]: rsi (child_user_rsp: Go в дочернем потоке восстанавливает RSP из RSI)
    // [9]: rdi (0)
    // [10]: user_rip (rcx для sysretq)
    // [11]: user_rsp (rsp для sysretq)
    let child_rsp = unsafe {
        let ptr = (kernel_stack_top - 96) as *mut u64;
        ptr.add(0).write(regs.r15);
        ptr.add(1).write(regs.r14);
        ptr.add(2).write(regs.r13);
        ptr.add(3).write(regs.r12);
        ptr.add(4).write(regs.rbx);
        ptr.add(5).write(regs.rbp);
        ptr.add(6).write(crate::arch::x86_64::syscall::ret_from_fork as *const () as usize as u64);
        ptr.add(7).write(regs.r9);
        ptr.add(8).write(child_user_rsp);
        ptr.add(9).write(0);
        ptr.add(10).write(child_user_rip);
        ptr.add(11).write(child_user_rsp);
        ptr as *mut u8
    };

    if parent_tidptr != 0 && parent_tidptr < super::USER_SPACE_END {
        unsafe {
            core::ptr::write_volatile(parent_tidptr as *mut u32, child_tid.0 as u32);
        }
    }

    let clear_tid = if (flags & 0x00200000) != 0 || child_tidptr != 0 {
        if child_tidptr != 0 && child_tidptr < super::USER_SPACE_END {
            unsafe {
                core::ptr::write_volatile(child_tidptr as *mut u32, child_tid.0 as u32);
            }
        }
        child_tidptr
    } else {
        0
    };

    let is_same_thread_group = (flags & 0x00010000) != 0;
    let (child_tgid, parent_tgid) = if is_same_thread_group {
        // CLONE_THREAD: тот же процесс
        (crate::task::current_thread_tgid(), crate::task::current_thread_parent_tgid())
    } else {
        // Новый процесс: уникальный TGID из глобального счетчика (не коллидирует с PID 1 init)
        (NEXT_PID.fetch_add(1, Ordering::Relaxed), crate::task::current_thread_tgid())
    };
    let child_pgid = crate::task::current_thread_pgid();

    if (flags & 0x00000400) != 0 {
        crate::fs::share_fd_table(parent_tgid, child_tgid);
    } else {
        crate::fs::clone_fd_table(parent_tgid, child_tgid);
    }

    if !is_same_thread_group {
        super::clone_process_cwd(parent_tgid, child_tgid);
    }

    let child_cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) child_cr3);
    }

    let mut child_thread = alloc::boxed::Box::new(crate::task::Thread::new_clone(
        child_tid,
        child_tgid,
        parent_tgid,
        child_pgid,
        stack,
        kernel_stack_top,
        child_rsp,
        child_cr3,
        child_fs_base,
        clear_tid,
        crate::task::current_thread_uid(),
        crate::task::current_thread_gid(),
        crate::task::current_thread_euid(),
        crate::task::current_thread_egid(),
        crate::task::current_thread_suid(),
        crate::task::current_thread_sgid(),
        crate::task::current_thread_umask(),
    ));
    child_thread.is_main = !is_same_thread_group;

    crate::task::add_thread(child_thread);

    child_tid.0 as u64
}

pub fn sys_fork(regs: &SyscallRegisters) -> u64 {
    // SIGCHLD = 17
    sys_clone(17, 0, 0, 0, 0, regs)
}

pub fn sys_vfork(regs: &SyscallRegisters) -> u64 {
    // CLONE_VFORK (0x4000) | CLONE_VM (0x100) | SIGCHLD (17)
    sys_clone(0x00004000 | 0x00000100 | 17, 0, 0, 0, 0, regs)
}

pub fn sys_getppid() -> u64 {
    crate::task::current_thread_parent_tgid()
}

pub fn sys_getpgrp() -> u64 {
    crate::task::current_thread_pgid()
}

pub fn sys_getpgid(pid: u64) -> u64 {
    if pid == 0 || pid == crate::task::current_thread_tgid() {
        crate::task::current_thread_pgid()
    } else {
        crate::task::current_thread_pgid()
    }
}

pub fn sys_setpgid(pid: u64, pgid: u64) -> u64 {
    let cur_tgid = crate::task::current_thread_tgid();
    if pid == 0 || pid == cur_tgid {
        let new_pgid = if pgid == 0 { cur_tgid } else { pgid };
        crate::task::set_current_thread_pgid(new_pgid);
        0
    } else {
        (-(super::ESRCH as i64)) as u64
    }
}


pub fn sys_nanosleep(req_ptr: u64, _rem_ptr: u64) -> u64 {
    if !validate_user_buffer(req_ptr, 16) {
        return (-(EFAULT as i64)) as u64;
    }
    let req = unsafe { core::ptr::read_unaligned(req_ptr as *const crate::task::futex::Timespec) };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return (-(EINVAL as i64)) as u64;
    }
    let delta = (req.tv_sec as u64).saturating_mul(100) + (req.tv_nsec as u64 + 9_999_999) / 10_000_000;
    let deadline = crate::arch::x86_64::timer::ticks().saturating_add(delta);
    crate::task::block_current(Some(deadline));
    if crate::task::signal::is_sigint_pending_for_current() {
        return (-(EINTR as i64)) as u64;
    }
    0
}


static REALTIME_BOOT_OFFSET: AtomicI64 = AtomicI64::new(0);
static RTC_SYNCED: AtomicBool = AtomicBool::new(false);

fn get_realtime_epoch() -> (i64, i64) {
    let ticks = crate::arch::x86_64::timer::ticks();
    if !RTC_SYNCED.load(Ordering::Relaxed) {
        let dt = crate::drivers::rtc::read_datetime();
        let rtc_epoch = crate::drivers::rtc::rtc_to_epoch(&dt);
        let boot_offset = rtc_epoch - (ticks / 100) as i64;
        REALTIME_BOOT_OFFSET.store(boot_offset, Ordering::Relaxed);
        RTC_SYNCED.store(true, Ordering::Relaxed);
    }
    let offset = REALTIME_BOOT_OFFSET.load(Ordering::Relaxed);
    let sec = offset + (ticks / 100) as i64;
    let nsec = ((ticks % 100) * 10_000_000) as i64;
    (sec, nsec)
}

pub fn sys_clock_gettime(clock_id: u64, tp_ptr: u64) -> u64 {
    if !validate_user_buffer(tp_ptr, 16) {
        return (-(EFAULT as i64)) as u64;
    }
    let ticks = crate::arch::x86_64::timer::ticks();
    let (sec, nsec) = match clock_id {
        0 /* CLOCK_REALTIME */ | 5 /* CLOCK_REALTIME_COARSE */ => get_realtime_epoch(),
        _ /* CLOCK_MONOTONIC, CLOCK_BOOTTIME, etc. */ => {
            let s = (ticks / 100) as i64;
            let ns = ((ticks % 100) * 10_000_000) as i64;
            (s, ns)
        }
    };

    let ts = crate::task::futex::Timespec {
        tv_sec: sec,
        tv_nsec: nsec,
    };
    unsafe {
        core::ptr::write_unaligned(tp_ptr as *mut crate::task::futex::Timespec, ts);
    }
    0
}

pub fn sys_clock_settime(clock_id: u64, tp_ptr: u64) -> u64 {
    if !validate_user_buffer(tp_ptr, 16) {
        return (-(EFAULT as i64)) as u64;
    }
    if crate::task::syscall::creds::sys_getuid() != 0 {
        return (-(EPERM as i64)) as u64;
    }
    if clock_id != 0 {
        return (-(EINVAL as i64)) as u64;
    }
    let ts = unsafe { core::ptr::read_unaligned(tp_ptr as *const crate::task::futex::Timespec) };
    let ticks = crate::arch::x86_64::timer::ticks();
    let new_offset = ts.tv_sec - (ticks / 100) as i64;
    REALTIME_BOOT_OFFSET.store(new_offset, Ordering::Relaxed);
    RTC_SYNCED.store(true, Ordering::Relaxed);

    // Синхронизируем аппаратный чип CMOS RTC
    let dt = crate::drivers::rtc::epoch_to_rtc(ts.tv_sec);
    crate::drivers::rtc::write_datetime(&dt);
    0
}

pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64) -> u64 {
    if tv_ptr != 0 {
        if !validate_user_buffer(tv_ptr, 16) {
            return (-(EFAULT as i64)) as u64;
        }
        let (sec, nsec) = get_realtime_epoch();
        let tv = crate::task::futex::Timeval {
            tv_sec: sec,
            tv_usec: nsec / 1000,
        };
        unsafe {
            core::ptr::write_unaligned(tv_ptr as *mut crate::task::futex::Timeval, tv);
        }
    }
    if tz_ptr != 0 {
        if !validate_user_buffer(tz_ptr, 8) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe {
            let ptr = tz_ptr as *mut i32;
            ptr.write(-180); // tz_minuteswest (-180 = UTC+3)
            ptr.add(1).write(0); // tz_dsttime
        }
    }
    0
}

pub fn sys_settimeofday(tv_ptr: u64, _tz_ptr: u64) -> u64 {
    if crate::task::syscall::creds::sys_getuid() != 0 {
        return (-(EPERM as i64)) as u64;
    }
    if tv_ptr != 0 {
        if !validate_user_buffer(tv_ptr, 16) {
            return (-(EFAULT as i64)) as u64;
        }
        let tv = unsafe { core::ptr::read_unaligned(tv_ptr as *const crate::task::futex::Timeval) };
        let ticks = crate::arch::x86_64::timer::ticks();
        let new_offset = tv.tv_sec - (ticks / 100) as i64;
        REALTIME_BOOT_OFFSET.store(new_offset, Ordering::Relaxed);
        RTC_SYNCED.store(true, Ordering::Relaxed);

        // Синхронизируем аппаратный чип CMOS RTC
        let dt = crate::drivers::rtc::epoch_to_rtc(tv.tv_sec);
        crate::drivers::rtc::write_datetime(&dt);
    }
    0
}

pub const LINUX_REBOOT_MAGIC1: u64 = 0xfee1dead;
pub const LINUX_REBOOT_MAGIC2: u64 = 672274793;
pub const LINUX_REBOOT_MAGIC2A: u64 = 85072278;
pub const LINUX_REBOOT_MAGIC2B: u64 = 369367448;
pub const LINUX_REBOOT_MAGIC2C: u64 = 537993216;

pub const LINUX_REBOOT_CMD_RESTART: u64 = 0x01234567;
pub const LINUX_REBOOT_CMD_HALT: u64 = 0xcdef0123;
pub const LINUX_REBOOT_CMD_POWER_OFF: u64 = 0x4321fedc;
pub const LINUX_REBOOT_CMD_RESTART2: u64 = 0xa1b2c3d4;
pub const LINUX_REBOOT_CMD_CAD_ON: u64 = 0x89abcdef;
pub const LINUX_REBOOT_CMD_CAD_OFF: u64 = 0x00000000;

pub fn sys_reboot(magic1: u64, magic2: u64, cmd: u64, _arg: u64) -> u64 {
    if magic1 != LINUX_REBOOT_MAGIC1
        || (magic2 != LINUX_REBOOT_MAGIC2
            && magic2 != LINUX_REBOOT_MAGIC2A
            && magic2 != LINUX_REBOOT_MAGIC2B
            && magic2 != LINUX_REBOOT_MAGIC2C)
    {
        return (-(EINVAL as i64)) as u64;
    }

    if crate::task::syscall::creds::sys_getuid() != 0 {
        return (-(EPERM as i64)) as u64;
    }

    match cmd {
        LINUX_REBOOT_CMD_POWER_OFF => {
            crate::serial_println!("wunix: poweroff requested by PID {}", crate::task::current_thread_tgid());
            crate::println!("{}", crate::i18n::t("Завершение работы операционной системы...", "Shutting down operating system..."));
            crate::arch::x86_64::cpu::poweroff();
        }
        LINUX_REBOOT_CMD_RESTART | LINUX_REBOOT_CMD_RESTART2 => {
            crate::serial_println!("wunix: restart requested by PID {}", crate::task::current_thread_tgid());
            crate::println!("{}", crate::i18n::t("Перезагрузка операционной системы...", "Restarting operating system..."));
            crate::arch::x86_64::cpu::reboot();
        }
        LINUX_REBOOT_CMD_HALT => {
            crate::serial_println!("wunix: halt requested by PID {}", crate::task::current_thread_tgid());
            crate::println!("{}", crate::i18n::t("Система остановлена.", "System halted."));
            crate::arch::x86_64::cpu::halt();
        }
        LINUX_REBOOT_CMD_CAD_ON | LINUX_REBOOT_CMD_CAD_OFF => 0,
        _ => (-(EINVAL as i64)) as u64,
    }
}


pub fn sys_sched_getaffinity(_pid: u64, len: u64, mask_ptr: u64) -> u64 {
    if !validate_user_buffer(mask_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len < 8 {
        return (-(EINVAL as i64)) as u64;
    }
    unsafe {
        let ptr = mask_ptr as *mut u64;
        core::ptr::write_bytes(mask_ptr as *mut u8, 0, len as usize);
        *ptr = 1; // 1 активное процессорное ядро (CPU 0)
    }
    8
}


pub fn sys_getcwd(buf_ptr: u64, size: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, size) || size == 0 {
        return (-(EFAULT as i64)) as u64;
    }

    let cwd = get_cwd();
    let bytes = cwd.as_bytes();

    if bytes.len() + 1 > size as usize {
        return (-(ERANGE as i64)) as u64;
    }

    unsafe {
        let dst = buf_ptr as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        *dst.add(bytes.len()) = 0;
    }

    (bytes.len() + 1) as u64
}


pub fn sys_chdir(path_ptr: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let target_path = resolve_relative_path(&path);

    match crate::fs::resolve_path(&target_path, true) {
        Ok(node) => {
            if node.metadata().itype != crate::fs::InodeType::Directory {
                return (-(ENOTDIR as i64)) as u64;
            }
            set_cwd(&crate::fs::path::normalize_path(&target_path));
            0
        }
        Err(_) => (-(ENOENT as i64)) as u64,
    }
}

pub fn sys_chroot(path_ptr: u64) -> u64 {
    let path = match read_user_cstr(path_ptr, 1024) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };
    let target_path = resolve_relative_path(&path);
    match crate::fs::resolve_path(&target_path, true) {
        Ok(node) => {
            if node.metadata().itype != crate::fs::InodeType::Directory {
                return (-(ENOTDIR as i64)) as u64;
            }
            0
        }
        Err(_) => (-(ENOENT as i64)) as u64,
    }
}


pub fn sys_uname(uts_ptr: u64) -> u64 {
    const UTS_FIELD_SIZE: usize = 65;
    const UTS_TOTAL_SIZE: usize = UTS_FIELD_SIZE * 6;

    if !validate_user_buffer(uts_ptr, UTS_TOTAL_SIZE as u64) {
        return (-(EFAULT as i64)) as u64;
    }

    let fields: [&[u8]; 6] = [
        b"Wunix\0",
        b"wunix\0",
        b"0.1.0-wunix\0",
        b"#1 SMP PREEMPT Wunix 2026 x86_64\0",
        b"x86_64\0",
        b"(none)\0",
    ];

    unsafe {
        let base = uts_ptr as *mut u8;
        core::ptr::write_bytes(base, 0, UTS_TOTAL_SIZE);

        for (i, field) in fields.iter().enumerate() {
            let offset = i * UTS_FIELD_SIZE;
            let len = field.len().min(UTS_FIELD_SIZE);
            core::ptr::copy_nonoverlapping(field.as_ptr(), base.add(offset), len);
        }
    }

    0
}


pub fn sys_sysinfo(info_ptr: u64) -> u64 {
    if !validate_user_buffer(info_ptr, 112) {
        return (-(EFAULT as i64)) as u64;
    }

    unsafe {
        let base = info_ptr as *mut u8;
        core::ptr::write_bytes(base, 0, 112);

        let uptime: i64 = (crate::arch::x86_64::timer::ticks() / 100) as i64;
        core::ptr::write_unaligned(base.add(0) as *mut i64, uptime);

        let totalram: u64 = 512 * 1024 * 1024;
        core::ptr::write_unaligned(base.add(32) as *mut u64, totalram);

        let freeram: u64 = 450 * 1024 * 1024;
        core::ptr::write_unaligned(base.add(40) as *mut u64, freeram);

        let bufferram: u64 = 16 * 1024 * 1024;
        core::ptr::write_unaligned(base.add(56) as *mut u64, bufferram);

        core::ptr::write_unaligned(base.add(80) as *mut u16, 2);
        core::ptr::write_unaligned(base.add(100) as *mut u32, 1);
    }

    0
}


pub fn sys_execve(path_ptr: u64, argv_ptr: u64, envp_ptr: u64) -> u64 {
    let path_str = match read_user_cstr(path_ptr, 512) {
        Ok(p) => p,
        Err(err) => return (-(err as i64)) as u64,
    };

    let mut args: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    if argv_ptr != 0 && validate_user_buffer(argv_ptr, 8) {
        let mut cur_argv = argv_ptr as *const u64;
        let mut count = 0;
        loop {
            if (cur_argv as u64) >= super::USER_SPACE_END || count >= 64 {
                break;
            }
            let arg_str_ptr = unsafe { *cur_argv };
            if arg_str_ptr == 0 {
                break;
            }
            match read_user_cstr(arg_str_ptr, 1024) {
                Ok(s) => args.push(s),
                Err(_) => break,
            }
            cur_argv = unsafe { cur_argv.add(1) };
            count += 1;
        }
    }
    if args.is_empty() {
        args.push(path_str.clone());
    }

    let mut envp: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    if envp_ptr != 0 && validate_user_buffer(envp_ptr, 8) {
        let mut cur_env = envp_ptr as *const u64;
        let mut count = 0;
        loop {
            if (cur_env as u64) >= super::USER_SPACE_END || count >= 64 {
                break;
            }
            let env_str_ptr = unsafe { *cur_env };
            if env_str_ptr == 0 {
                break;
            }
            match read_user_cstr(env_str_ptr, 1024) {
                Ok(s) => envp.push(s),
                Err(_) => break,
            }
            cur_env = unsafe { cur_env.add(1) };
            count += 1;
        }
    }
    if envp.is_empty() {
        envp.push(String::from("PATH=/bin:/usr/bin"));
        envp.push(String::from("USER=root"));
        envp.push(String::from("HOME=/root"));
        envp.push(String::from("TERM=xterm-256color"));
    }

    let candidates = [
        resolve_relative_path(&path_str),
        if !path_str.starts_with('/') {
            alloc::format!("/bin/{}", path_str)
        } else {
            String::new()
        },
        if !path_str.starts_with('/') {
            alloc::format!("/usr/bin/{}", path_str)
        } else {
            String::new()
        },
        if !path_str.starts_with('/') {
            alloc::format!("/win/drives/c/Windows/System32/{}", path_str)
        } else {
            String::new()
        },
    ];

    let mut binary_data = None;
    let mut resolved_node = None;
    for cand in &candidates {
        if cand.is_empty() {
            continue;
        }
        if let Ok(node) = crate::fs::resolve_path(cand, true) {
            if let Ok(data) = crate::fs::read_file(cand) {
                binary_data = Some(data);
                resolved_node = Some(node);
                break;
            }
        }
    }

    let data = match binary_data {
        Some(d) => d,
        None => return (-(ENOENT as i64)) as u64,
    };
    let node = match resolved_node {
        Some(n) => n,
        None => return (-(ENOENT as i64)) as u64,
    };
    drop(candidates);

    // Проверка прав на исполнение бинарного файла (флаг 'x')
    if let Err(_) = crate::fs::check_permission(&node, false, false, true) {
        return (-(EACCES as i64)) as u64;
    }

    let meta = node.metadata();
    let old_uid = crate::task::current_thread_uid();
    let old_gid = crate::task::current_thread_gid();
    let old_euid = crate::task::current_thread_euid();
    let old_egid = crate::task::current_thread_egid();
    let old_suid = crate::task::current_thread_suid();
    let old_sgid = crate::task::current_thread_sgid();

    // Применение механизмов SUID (0o4000) и SGID (0o2000)
    let child_uid = old_uid;
    let child_gid = old_gid;
    let mut child_euid = old_euid;
    let mut child_egid = old_egid;
    let mut child_suid = old_suid;
    let mut child_sgid = old_sgid;

    if (meta.permissions & 0o4000) != 0 {
        child_euid = meta.uid;
        child_suid = meta.uid;
    }
    if (meta.permissions & 0o2000) != 0 {
        child_egid = meta.gid;
        child_sgid = meta.gid;
    }

    let (mut user_space, elf_info) = if crate::task::pe::is_pe(&data) {
        match crate::task::pe::load_pe(&data) {
            Ok(res) => res,
            Err(e) => {
                serial_println!("[execve] PE load error for '{}': {:?}", path_str, e);
                return (-(ENOEXEC as i64)) as u64;
            }
        }
    } else {
        match crate::task::elf::load_elf(&data) {
            Ok(res) => res,
            Err(e) => {
                serial_println!("[execve] ELF load error for '{}': {:?}", path_str, e);
                return (-(ENOEXEC as i64)) as u64;
            }
        }
    };
    drop(data);
    let is_shell = is_shell_binary_path(&path_str);
    drop(path_str);

    let (stack_top, argv_addr) = match crate::task::elf::setup_user_stack(&mut user_space, &args, &envp, &elf_info) {
        Ok(res) => res,
        Err(_) => return (-(ENOMEM as i64)) as u64,
    };
    let argc = args.len() as u64;
    drop(args);
    drop(envp);

    let pml4_phys = user_space.pml4_frame.start_address().as_u64();
    drop(user_space);

    let brk_start = (elf_info.max_vaddr + 4095) & !4095;
    super::mem::reset_process_vma(brk_start);

    let parent_tgid = crate::task::current_thread_tgid();
    let child_tgid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::fs::clone_fd_table(parent_tgid, child_tgid);
    crate::task::set_current_thread_tgid(child_tgid);
    super::clone_process_cwd(parent_tgid, child_tgid);

    // Выделяем собственный изолированный стек ядра (32 КиБ) для дочернего процесса,
    // чтобы его системные вызовы и аллокации не повреждали сохраненный стек родителя
    let (_child_kstack, child_kstack_top) = crate::task::syscall::alloc_kernel_stack();
    let old_kstack_top = crate::task::current_thread_kernel_stack_top();
    crate::task::set_current_thread_kernel_stack_top(child_kstack_top);
    crate::arch::x86_64::gdt::set_tss_rsp0(child_kstack_top);
    crate::arch::x86_64::syscall::set_kernel_stack(child_kstack_top);

    crate::task::set_current_thread_uid(child_uid, child_euid, child_suid);
    crate::task::set_current_thread_gid(child_gid, child_egid, child_sgid);

    let cur_depth = crate::arch::x86_64::syscall::current_child_stack_depth() as usize;
    let next_depth = cur_depth + 1;
    set_child_is_shell(next_depth, is_shell);

    // Сбрасываем старый сигнал SIGINT перед запуском нового процесса
    crate::task::signal::clear_sigint();

    // Сохраняем FS_BASE родителя и очищаем его для дочернего процесса,
    // чтобы изолировать TLS и предотвратить Page Fault от устаревших указателей
    let old_fs_base = unsafe { crate::arch::x86_64::syscall::read_fs_base() };
    unsafe { crate::arch::x86_64::syscall::write_fs_base(0); }
    crate::task::set_current_fs_base(0);

    let prev_sync = crate::task::is_current_thread_sync_child();
    crate::task::set_current_thread_sync_child(true);

    let prev_fg = crate::task::signal::foreground_tgid();
    crate::task::signal::set_foreground_tgid(child_tgid);

    let exit_code = unsafe {
        crate::arch::x86_64::syscall::enter_user_child(
            elf_info.entry_point.as_u64(),
            stack_top,
            pml4_phys,
            argc,
            argv_addr,
            child_kstack_top,
        )
    };

    crate::task::signal::set_foreground_tgid(prev_fg);
    crate::task::set_current_thread_sync_child(prev_sync);

    // Восстанавливаем FS_BASE родительского процесса
    unsafe { crate::arch::x86_64::syscall::write_fs_base(old_fs_base); }
    crate::task::set_current_fs_base(old_fs_base);

    set_child_is_shell(next_depth, false);

    crate::task::set_current_thread_uid(old_uid, old_euid, old_suid);
    crate::task::set_current_thread_gid(old_gid, old_egid, old_sgid);
    crate::task::set_current_thread_kernel_stack_top(old_kstack_top);
    crate::arch::x86_64::gdt::set_tss_rsp0(old_kstack_top);
    crate::arch::x86_64::syscall::set_kernel_stack(old_kstack_top);
    crate::fs::remove_process_fd_table(child_tgid);
    super::remove_process_cwd(child_tgid);
    crate::task::set_current_thread_tgid(parent_tgid);

    exit_code
}


pub fn sys_yield() -> u64 {
    crate::task::yield_now();
    0
}


pub fn sys_exit(status: u64) -> ! {
    let is_sync = crate::task::is_current_thread_sync_child();
    let has_parent = unsafe { crate::arch::x86_64::syscall::has_parent_process() };
    if (is_sync && has_parent) || crate::task::is_current_main_thread() {
        sys_exit_group(status);
    } else {
        crate::task::exit();
    }
}


pub fn sys_exit_group(status: u64) -> ! {
    let cur_tgid = crate::task::current_thread_tgid();
    let cur_tid = crate::task::current_thread_id();
    let is_sync = crate::task::is_current_thread_sync_child();
    let has_parent = unsafe { crate::arch::x86_64::syscall::has_parent_process() };

    // Удаляем все остальные потоки этого процесса из очередей планировщика
    crate::task::terminate_process_threads(cur_tgid, cur_tid);
    crate::fs::remove_process_fd_table(cur_tgid);
    super::remove_process_cwd(cur_tgid);

    if is_sync && has_parent {
        unsafe {
            crate::arch::x86_64::syscall::resume_parent_process(status);
        }
    } else {
        crate::task::exit();
    }
}

pub fn sys_wait4(pid: i64, status_ptr: u64, _options: u64, _rusage_ptr: u64) -> u64 {
    if status_ptr != 0 && validate_user_buffer(status_ptr, 4) {
        unsafe {
            *(status_ptr as *mut i32) = 0;
        }
    }
    if pid > 0 {
        pid as u64
    } else {
        1
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RUsage {
    pub ru_utime: Timeval,
    pub ru_stime: Timeval,
    pub ru_maxrss: i64,
    pub ru_ixrss: i64,
    pub ru_idrss: i64,
    pub ru_isrss: i64,
    pub ru_minflt: i64,
    pub ru_majflt: i64,
    pub ru_nswap: i64,
    pub ru_inblock: i64,
    pub ru_oublock: i64,
    pub ru_msgsnd: i64,
    pub ru_msgrcv: i64,
    pub ru_nsignals: i64,
    pub ru_nvcsw: i64,
    pub ru_nivcsw: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Tms {
    pub tms_utime: i64,
    pub tms_stime: i64,
    pub tms_cutime: i64,
    pub tms_cstime: i64,
}

pub fn sys_getrusage(_who: i32, usage_ptr: u64) -> u64 {
    if !validate_user_buffer(usage_ptr, core::mem::size_of::<RUsage>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }
    let ticks = crate::arch::x86_64::timer::ticks();
    let sec = (ticks / 100) as i64;
    let usec = ((ticks % 100) * 10_000) as i64;

    let mut usage = RUsage::default();
    usage.ru_utime = Timeval { tv_sec: sec, tv_usec: usec };
    usage.ru_stime = Timeval { tv_sec: sec / 2, tv_usec: usec / 2 };
    usage.ru_maxrss = 4096; // 4 МиБ базовый RSS

    unsafe {
        core::ptr::write_unaligned(usage_ptr as *mut RUsage, usage);
    }
    0
}

pub fn sys_times(buf_ptr: u64) -> u64 {
    let ticks = crate::arch::x86_64::timer::ticks() as i64;
    if buf_ptr != 0 {
        if !validate_user_buffer(buf_ptr, core::mem::size_of::<Tms>() as u64) {
            return (-(EFAULT as i64)) as u64;
        }
        let tms = Tms {
            tms_utime: ticks,
            tms_stime: ticks / 2,
            tms_cutime: 0,
            tms_cstime: 0,
        };
        unsafe {
            core::ptr::write_unaligned(buf_ptr as *mut Tms, tms);
        }
    }
    ticks as u64
}

pub fn sys_prctl(option: u64, arg2: u64, _arg3: u64, _arg4: u64, _arg5: u64) -> u64 {
    const PR_SET_PDEATHSIG: u64 = 1;
    const PR_GET_PDEATHSIG: u64 = 2;
    const PR_GET_DUMPABLE: u64 = 3;
    const PR_SET_DUMPABLE: u64 = 4;
    const PR_SET_NAME: u64 = 15;
    const PR_GET_NAME: u64 = 16;

    match option {
        PR_SET_NAME => {
            if arg2 != 0 && validate_user_buffer(arg2, 16) {
                0
            } else {
                (-(EFAULT as i64)) as u64
            }
        }
        PR_GET_NAME => {
            if arg2 != 0 && validate_user_buffer(arg2, 16) {
                let name = b"wunix_task\0\0\0\0\0\0";
                unsafe {
                    core::ptr::copy_nonoverlapping(name.as_ptr(), arg2 as *mut u8, 16);
                }
                0
            } else {
                (-(EFAULT as i64)) as u64
            }
        }
        PR_GET_DUMPABLE => 1,
        PR_SET_DUMPABLE | PR_SET_PDEATHSIG => 0,
        PR_GET_PDEATHSIG => {
            if arg2 != 0 && validate_user_buffer(arg2, 4) {
                unsafe { core::ptr::write_unaligned(arg2 as *mut i32, 0); }
                0
            } else {
                (-(EFAULT as i64)) as u64
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_is_shell_binary_path() {
        assert!(is_shell_binary_path("/bin/wsh"));
        assert!(is_shell_binary_path("wsh"));
        assert!(is_shell_binary_path("/bin/login"));
        assert!(is_shell_binary_path("login"));
        assert!(is_shell_binary_path("/bin/init"));
        assert!(is_shell_binary_path("init"));
        assert!(is_shell_binary_path("/bin/sh"));
        assert!(is_shell_binary_path("sh"));

        assert!(!is_shell_binary_path("/bin/cat"));
        assert!(!is_shell_binary_path("cat"));
        assert!(!is_shell_binary_path("/bin/ls"));
        assert!(!is_shell_binary_path("/bin/sleep"));
        assert!(!is_shell_binary_path("ping"));
    }

    #[test_case]
    fn test_child_shell_mask_depths() {
        set_child_is_shell(1, true);
        assert!((CHILD_SHELL_MASK.load(Ordering::SeqCst) & (1 << 1)) != 0);

        set_child_is_shell(2, false);
        assert!((CHILD_SHELL_MASK.load(Ordering::SeqCst) & (1 << 2)) == 0);

        set_child_is_shell(1, false);
        assert!((CHILD_SHELL_MASK.load(Ordering::SeqCst) & (1 << 1)) == 0);
    }
}
