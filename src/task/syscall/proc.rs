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
    name == "wsh" || name == "sh" || name == "bash" || name == "login" || name == "init"
}

static NEXT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(10);

const CLONE_VM: u64 = 0x0000_0100;
const CLONE_FILES: u64 = 0x0000_0400;
const CLONE_THREAD: u64 = 0x0001_0000;
const CLONE_SETTLS: u64 = 0x0008_0000;
const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CloneArgs {
    pub flags: u64,
    pub pidfd: u64,
    pub child_tid: u64,
    pub parent_tid: u64,
    pub exit_signal: u64,
    pub stack: u64,
    pub stack_size: u64,
    pub tls: u64,
    pub set_tid: u64,
    pub set_tid_size: u64,
    pub cgroup: u64,
}

fn do_clone(
    flags: u64,
    newsp: u64,
    parent_tidptr: u64,
    child_tidptr: u64,
    tls: u64,
    regs: &SyscallRegisters,
    is_clone3: bool,
) -> u64 {
    let child_tid = crate::task::ThreadId::new();
    let (stack, kernel_stack_top) = crate::task::syscall::alloc_kernel_stack();

    let child_user_rsp = if newsp != 0 { newsp } else { regs.user_rsp };
    let child_user_rip = regs.user_rip;

    let child_fs_base = if (flags & CLONE_SETTLS) != 0 || tls != 0 {
        tls
    } else {
        crate::task::get_current_fs_base()
    };

    // Для legacy SYS_CLONE рантайм Go ожидает child_user_rsp в RSI; для clone3
    // сохраняем все пользовательские caller-saved регистры (включая rdx/r8).
    let child_rsi = if is_clone3 { regs.rsi } else { child_user_rsp };

    // Начальный стек-фрейм (120 байт = 15 * 8) для context_switch и ret_from_fork
    let child_rsp = unsafe {
        let ptr = (kernel_stack_top - 120) as *mut u64;
        ptr.add(0).write(regs.r15);
        ptr.add(1).write(regs.r14);
        ptr.add(2).write(regs.r13);
        ptr.add(3).write(regs.r12);
        ptr.add(4).write(regs.rbx);
        ptr.add(5).write(regs.rbp);
        ptr.add(6).write(crate::arch::x86_64::syscall::ret_from_fork as *const () as usize as u64);
        ptr.add(7).write(regs.r10);
        ptr.add(8).write(regs.r8);
        ptr.add(9).write(regs.r9);
        ptr.add(10).write(regs.rdx);
        ptr.add(11).write(child_rsi);
        ptr.add(12).write(regs.rdi);
        ptr.add(13).write(child_user_rip);
        ptr.add(14).write(child_user_rsp);
        ptr as *mut u8
    };

    let is_same_thread_group = (flags & CLONE_THREAD) != 0;
    let (child_tgid, parent_tgid) = if is_same_thread_group {
        // CLONE_THREAD: поток того же процесса, таблица дескрипторов уже общая
        (crate::task::current_thread_tgid(), crate::task::current_thread_parent_tgid())
    } else {
        (NEXT_PID.fetch_add(1, Ordering::Relaxed), crate::task::current_thread_tgid())
    };
    let child_pgid = crate::task::current_thread_pgid();
    let reported_child_id = if is_same_thread_group {
        1_000_000 + child_tid.0
    } else {
        child_tgid
    };

    let parent_cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) parent_cr3);
    }

    let shares_vm = (flags & CLONE_VM) != 0;
    let mut cloned_space = if shares_vm {
        None
    } else {
        match crate::memory::UserAddressSpace::clone_from_cr3(parent_cr3) {
            Ok(space) => Some(space),
            Err(_) => return (-(ENOMEM as i64)) as u64,
        }
    };
    let child_cr3 = match &cloned_space {
        Some(space) => space.pml4_frame.start_address().as_u64(),
        None => parent_cr3,
    };

    if (flags & CLONE_PARENT_SETTID) != 0
        && parent_tidptr != 0
        && validate_user_buffer(parent_tidptr, 4)
    {
        unsafe {
            core::ptr::write_volatile(parent_tidptr as *mut u32, reported_child_id as u32);
        }
    }

    if (flags & CLONE_CHILD_SETTID) != 0
        && child_tidptr != 0
        && validate_user_buffer(child_tidptr, 4)
    {
        if let Some(ref mut space) = cloned_space {
            let _ = space.write_user_bytes(child_tidptr, &(reported_child_id as u32).to_ne_bytes());
        } else {
            unsafe {
                core::ptr::write_volatile(child_tidptr as *mut u32, reported_child_id as u32);
            }
        }
    }

    let clear_tid = if (flags & CLONE_CHILD_CLEARTID) != 0
        && child_tidptr != 0
        && validate_user_buffer(child_tidptr, 4)
    {
        child_tidptr
    } else {
        0
    };

    if !is_same_thread_group {
        if (flags & CLONE_FILES) != 0 {
            crate::fs::share_fd_table(parent_tgid, child_tgid);
        } else {
            crate::fs::clone_fd_table(parent_tgid, child_tgid);
        }
        super::clone_process_cwd(parent_tgid, child_tgid);
        if !shares_vm {
            super::mem::clone_process_vma(parent_tgid, child_tgid);
        }
    }

    let mut child_thread = alloc::boxed::Box::new(crate::task::Thread::new_clone(
        child_tid,
        crate::task::current_thread_name(),
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
        crate::task::current_thread_abi(),
        crate::task::current_thread_exe_path(),
    ));
    child_thread.is_main = !is_same_thread_group;
    child_thread.is_fork_child = !is_same_thread_group;
    child_thread.cr3 = match cloned_space {
        Some(space) => space.into_raw_cr3(),
        None => parent_cr3,
    };
    child_thread.owns_cr3 = !shares_vm;
    child_thread.fpu = crate::arch::x86_64::cpu::FpuState::save_current();

    crate::task::add_thread(child_thread);

    reported_child_id
}

pub fn sys_clone(
    flags: u64,
    newsp: u64,
    parent_tidptr: u64,
    child_tidptr: u64,
    tls: u64,
    regs: &SyscallRegisters,
) -> u64 {
    do_clone(flags, newsp, parent_tidptr, child_tidptr, tls, regs, false)
}

pub fn sys_clone3(uargs_ptr: u64, size: u64, regs: &SyscallRegisters) -> u64 {
    const CLONE_ARGS_SIZE_VER0: u64 = 64;
    if size < CLONE_ARGS_SIZE_VER0 || size > 4096 {
        return (-(EINVAL as i64)) as u64;
    }
    if !validate_user_buffer(uargs_ptr, size) {
        return (-(EFAULT as i64)) as u64;
    }

    let mut args = CloneArgs::default();
    let copy_len = (size as usize).min(core::mem::size_of::<CloneArgs>());
    unsafe {
        core::ptr::copy_nonoverlapping(
            uargs_ptr as *const u8,
            (&mut args as *mut CloneArgs) as *mut u8,
            copy_len,
        );
    }

    // В clone3 передаются нижняя граница стека и его размер; на x86_64 стек растёт вниз
    let newsp = if args.stack != 0 {
        args.stack.saturating_add(args.stack_size)
    } else {
        0
    };

    do_clone(
        args.flags,
        newsp,
        args.parent_tid,
        args.child_tid,
        args.tls,
        regs,
        true,
    )
}

pub fn sys_fork(regs: &SyscallRegisters) -> u64 {
    // SIGCHLD = 17
    sys_clone(17, 0, 0, 0, 0, regs)
}

pub fn sys_vfork(regs: &SyscallRegisters) -> u64 {
    // Без приостановки родителя до execve разделение стека с CLONE_VM портит кадр вызова.
    // Полное клонирование адресного пространства (fork) сохраняет совместимость с семантикой vfork.
    sys_fork(regs)
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

pub(crate) fn get_realtime_epoch() -> (i64, i64) {
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

pub fn sys_time(tloc_ptr: u64) -> u64 {
    let (sec, _) = get_realtime_epoch();
    if tloc_ptr != 0 {
        if !validate_user_buffer(tloc_ptr, 8) {
            return (-(EFAULT as i64)) as u64;
        }
        unsafe {
            core::ptr::write_unaligned(tloc_ptr as *mut i64, sec);
        }
    }
    sec as u64
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

        let totalram = crate::memory::total_memory_bytes();
        core::ptr::write_unaligned(base.add(32) as *mut u64, totalram);

        let freeram = crate::memory::free_memory_bytes();
        core::ptr::write_unaligned(base.add(40) as *mut u64, freeram);

        let bufferram: u64 = 16 * 1024 * 1024;
        core::ptr::write_unaligned(base.add(56) as *mut u64, bufferram);

        let procs = crate::task::active_process_count();
        core::ptr::write_unaligned(base.add(80) as *mut u16, procs);
        core::ptr::write_unaligned(base.add(100) as *mut u32, 1);
    }

    0
}


fn resolve_executable_file(
    path_str: &str,
    envp: &[String],
) -> Option<(String, alloc::vec::Vec<u8>, alloc::sync::Arc<dyn crate::fs::Inode>)> {
    let mut candidates: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    if path_str.contains('/') {
        candidates.push(resolve_relative_path(path_str));
    } else {
        let path_env = envp
            .iter()
            .find(|e| e.starts_with("PATH="))
            .map(|e| &e[5..])
            .unwrap_or("/bin:/usr/bin");

        for dir in path_env.split(':') {
            let trimmed = dir.trim().trim_end_matches('/');
            if !trimmed.is_empty() {
                candidates.push(alloc::format!("{}/{}", trimmed, path_str));
            }
        }

        candidates.push(alloc::format!("/win/drives/c/Windows/System32/{}", path_str));
    }

    for cand in candidates {
        if cand.is_empty() {
            continue;
        }
        if let Ok(node) = crate::fs::resolve_path(&cand, true) {
            if node.metadata().itype == crate::fs::InodeType::Directory {
                continue;
            }
            if let Ok(data) = crate::fs::read_file(&cand) {
                return Some((cand, data, node));
            }
        }
    }
    None
}

fn parse_shebang_line(data: &[u8]) -> Option<(String, Option<String>)> {
    let rest = data.strip_prefix(b"#!")?;
    let max_len = rest.len().min(256);
    let line_bytes = match rest[..max_len].iter().position(|&b| b == b'\n') {
        Some(idx) => &rest[..idx],
        None => &rest[..max_len],
    };
    let line_str = core::str::from_utf8(line_bytes).ok()?;
    let trimmed = line_str.trim_end_matches('\r').trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.find(|c: char| c == ' ' || c == '\t') {
        Some(pos) => {
            let interp = &trimmed[..pos];
            let opt = trimmed[pos..].trim();
            let opt_arg = if opt.is_empty() { None } else { Some(String::from(opt)) };
            Some((String::from(interp), opt_arg))
        }
        None => Some((String::from(trimmed), None)),
    }
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

    let mut cur_path = path_str;
    let mut final_data = None;
    let mut final_node = None;
    const MAX_SHEBANG_DEPTH: usize = 4;

    for depth in 0..=MAX_SHEBANG_DEPTH {
        let Some((resolved_path, data, node)) = resolve_executable_file(&cur_path, &envp) else {
            crate::serial_println!("[sys_execve] FAILED binary not found: {}", cur_path);
            return (-(ENOENT as i64)) as u64;
        };

        // Проверка прав на исполнение (флаг 'x') для скрипта или бинарника на каждом шаге
        if crate::fs::check_permission(&node, false, false, true).is_err() {
            crate::serial_println!("[sys_execve] FAILED permission denied: {}", resolved_path);
            return (-(EACCES as i64)) as u64;
        }

        if data.starts_with(b"#!") {
            if depth == MAX_SHEBANG_DEPTH {
                return (-(40i64)) as u64; // -ELOOP
            }
            let Some((interp, opt_arg)) = parse_shebang_line(&data) else {
                return (-(ENOEXEC as i64)) as u64;
            };
            let mut new_args = alloc::vec::Vec::with_capacity(args.len() + 2);
            new_args.push(interp.clone());
            if let Some(a) = opt_arg {
                new_args.push(a);
            }
            new_args.push(resolved_path);
            if args.len() > 1 {
                new_args.extend(args.into_iter().skip(1));
            }
            args = new_args;
            cur_path = interp;
            continue;
        }

        cur_path = resolved_path;
        final_data = Some(data);
        final_node = Some(node);
        break;
    }

    let path_str = cur_path;
    let data = final_data.unwrap();
    let node = final_node.unwrap();

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

    let (mut user_space, elf_info, child_abi) = if crate::task::pe::is_pe(&data) {
        match crate::task::pe::load_pe(&data) {
            Ok((us, info)) => (us, info, crate::task::abi::AbiPersonality::WindowsNt),
            Err(e) => {
                serial_println!("[execve] PE load error for '{}': {:?}", path_str, e);
                return (-(ENOEXEC as i64)) as u64;
            }
        }
    } else if crate::task::darwin::is_macho(&data) {
        match crate::task::darwin::load_macho(&data) {
            Ok((us, info)) => (us, info, crate::task::abi::AbiPersonality::Darwin),
            Err(e) => {
                serial_println!("[execve] Mach-O load error for '{}': {:?}", path_str, e);
                return (-(ENOEXEC as i64)) as u64;
            }
        }
    } else {
        match crate::task::elf::load_elf(&data) {
            Ok((us, info)) => (us, info, crate::task::abi::AbiPersonality::Linux),
            Err(e) => {
                serial_println!("[execve] ELF load error for '{}': {:?}", path_str, e);
                return (-(ENOEXEC as i64)) as u64;
            }
        }
    };
    drop(data);
    let old_name = crate::task::current_thread_name();
    let old_exe_path = crate::task::current_thread_exe_path();
    let bin_name = path_str.rsplit('/').next().unwrap_or(&path_str);
    crate::task::set_current_thread_name(bin_name.as_bytes());
    crate::task::set_current_thread_exe_path(&path_str);
    let is_shell = is_shell_binary_path(&path_str);
    drop(path_str);

    let (stack_top, argv_addr) = match crate::task::elf::setup_user_stack(&mut user_space, &args, &envp, &elf_info) {
        Ok(res) => res,
        Err(_) => return (-(ENOMEM as i64)) as u64,
    };
    let argc = args.len() as u64;
    drop(args);
    drop(envp);

    let pml4_phys = user_space.into_raw_cr3();

    let brk_start = (elf_info.max_vaddr + 4095) & !4095;

    // Для потомка после fork или POSIX-программы (например bash, делающей tail-call execve без fork)
    // заменяем образ текущего процесса на месте, не возвращаясь в вызывающий код.
    let in_place_exec = crate::task::is_current_thread_fork_child() || !uses_sync_execve_spawn(&old_exe_path);
    if in_place_exec {
        let cur_tgid = crate::task::current_thread_tgid();
        let old_cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) old_cr3);
        }
        let old_owns = crate::task::current_thread_owns_cr3();
        super::mem::reset_process_vma_for(cur_tgid, brk_start);
        crate::task::set_current_thread_uid(child_uid, child_euid, child_suid);
        crate::task::set_current_thread_gid(child_gid, child_egid, child_sgid);
        crate::task::set_current_thread_abi(child_abi);
        crate::task::set_current_thread_cr3(pml4_phys);
        crate::task::set_current_thread_owns_cr3(true);
        // Переключаемся на новый CR3 перед освобождением старого адресного пространства fork-потомка
        x86_64::instructions::interrupts::without_interrupts(|| unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) pml4_phys);
            if old_owns && old_cr3 != 0 && (old_cr3 & !0xFFF) != (pml4_phys & !0xFFF) {
                crate::memory::process::free_user_address_space(old_cr3);
            }
        });
        unsafe { crate::arch::x86_64::syscall::write_fs_base(0); }
        crate::task::set_current_fs_base(0);
        if crate::task::is_current_thread_fork_child() {
            crate::task::set_current_thread_fork_child(false);
            crate::task::set_current_thread_sync_child(false);
        }

        let kstack_top = crate::task::current_thread_kernel_stack_top();
        crate::arch::x86_64::gdt::set_tss_rsp0(kstack_top);
        crate::arch::x86_64::syscall::set_kernel_stack(kstack_top);
        crate::arch::x86_64::cpu::FpuState::initial().restore();

        unsafe {
            crate::arch::x86_64::syscall::jump_to_user_exec(
                elf_info.entry_point.as_u64(),
                stack_top,
                pml4_phys,
                argc,
                argv_addr,
                kstack_top,
            );
        }
    }

    let parent_tgid = crate::task::current_thread_tgid();
    let child_tgid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    super::mem::reset_process_vma_for(child_tgid, brk_start);
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

    let prev_termios = *crate::task::syscall::fs::CURRENT_TERMIOS.lock();

    let old_abi = crate::task::current_thread_abi();
    crate::task::set_current_thread_abi(child_abi);
    let old_fpu = crate::arch::x86_64::cpu::FpuState::save_current();
    crate::arch::x86_64::cpu::FpuState::initial().restore();

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

    old_fpu.restore();

    // Дочерний процесс завершился и CR3 восстановлен на родителя — возвращаем физические фреймы
    unsafe {
        crate::memory::process::free_user_address_space(pml4_phys);
    }

    if exit_code >= 128 {
        *crate::task::syscall::fs::CURRENT_TERMIOS.lock() = prev_termios;
    }

    crate::task::set_current_thread_abi(old_abi);
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
    crate::task::set_current_thread_name(&old_name);
    crate::task::set_current_thread_exe_path(&old_exe_path);

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


static PROCESS_EXIT_STATUS: spin::Mutex<alloc::collections::BTreeMap<u64, (u64, u64, i32)>> =
    spin::Mutex::new(alloc::collections::BTreeMap::new());

pub fn record_process_exit(tgid: u64, raw_status: i32) {
    let parent_tgid = crate::task::current_thread_parent_tgid();
    let pgid = crate::task::current_thread_pgid();
    PROCESS_EXIT_STATUS.lock().insert(tgid, (parent_tgid, pgid, raw_status));
}

pub fn sys_exit_group(status: u64) -> ! {
    let cur_tgid = crate::task::current_thread_tgid();
    let cur_tid = crate::task::current_thread_id();
    let is_sync = crate::task::is_current_thread_sync_child();
    let has_parent = unsafe { crate::arch::x86_64::syscall::has_parent_process() };

    if !(is_sync && has_parent) {
        let exit_status = (status as i32 & 0xff) << 8;
        record_process_exit(cur_tgid, exit_status);
    }

    // Удаляем все остальные потоки этого процесса из очередей планировщика
    crate::task::terminate_process_threads(cur_tgid, cur_tid);
    crate::fs::remove_process_fd_table(cur_tgid);
    super::remove_process_cwd(cur_tgid);
    super::mem::remove_process_vma(cur_tgid);

    if is_sync && has_parent {
        unsafe {
            crate::arch::x86_64::syscall::resume_parent_process(status);
        }
    } else {
        crate::task::exit();
    }
}

pub const WNOHANG: u64 = 1;
pub const WUNTRACED: u64 = 2;
pub const WCONTINUED: u64 = 8;

fn uses_sync_execve_spawn(exe_path: &str) -> bool {
    let bin = exe_path.rsplit('/').next().unwrap_or(exe_path);
    matches!(
        bin,
        "init" | "wsh" | "sh" | "env" | "login" | "sudo" | "su" | "system" | "winstall" | "time" | "[" | "test" | "unalias"
    )
}

fn pop_matching_exit_status(cur_tgid: u64, pid: i64, cur_pgid: u64) -> Option<(u64, i32)> {
    let mut map = PROCESS_EXIT_STATUS.lock();
    let mut matched_key = None;
    for (&child_tgid, &(parent_tgid, child_pgid, status)) in map.iter() {
        if parent_tgid == cur_tgid || cur_tgid == 1 {
            let pid_match = if pid > 0 {
                child_tgid == pid as u64
            } else if pid == -1 {
                true
            } else if pid == 0 {
                child_pgid == cur_pgid
            } else {
                child_pgid == (-pid) as u64
            };
            if pid_match {
                matched_key = Some((child_tgid, status));
                break;
            }
        }
    }
    if let Some((child_tgid, status)) = matched_key {
        map.remove(&child_tgid);
        Some((child_tgid, status))
    } else {
        None
    }
}

pub fn sys_wait4(pid: i64, status_ptr: u64, options: u64, _rusage_ptr: u64) -> u64 {
    let cur_tgid = crate::task::current_thread_tgid();
    let cur_pgid = crate::task::current_thread_pgid();

    loop {
        let mut found = pop_matching_exit_status(cur_tgid, pid, cur_pgid);

        if found.is_none() {
            let has_running_children = crate::task::has_child_process(cur_tgid, pid, cur_pgid);
            if !has_running_children {
                // Повторяем проверку после отпускания локов: потомок мог записать статус
                // и удалить себя из планировщика в окне между первой проверкой и has_child_process.
                found = pop_matching_exit_status(cur_tgid, pid, cur_pgid);
                if found.is_none() {
                    return (-(super::ECHILD as i64)) as u64;
                }
            }
        }

        if let Some((child_tgid, status)) = found {
            if status_ptr != 0 && validate_user_buffer(status_ptr, 4) {
                unsafe {
                    *(status_ptr as *mut i32) = status;
                }
            }
            return child_tgid;
        }

        if (options & WNOHANG) != 0 {
            return 0;
        }

        crate::task::yield_now();
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SigInfo {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code: i32,
    pub _pad0: i32,
    pub si_pid: u32,
    pub si_uid: u32,
    pub si_status: i32,
    pub _pad1: [u64; 12],
    pub _pad2: u32,
}

pub fn sys_waitid(idtype: u64, id: u64, infop_ptr: u64, options: u64, _rusage_ptr: u64) -> u64 {
    if infop_ptr != 0 && !validate_user_buffer(infop_ptr, core::mem::size_of::<SigInfo>() as u64) {
        return (-(EFAULT as i64)) as u64;
    }

    let pid = match idtype {
        0 /* P_ALL */ => -1,
        1 /* P_PID */ => id as i64,
        2 /* P_PGID */ => -(id as i64),
        _ => return (-(EINVAL as i64)) as u64,
    };

    let mut status: i32 = 0;
    let res = sys_wait4(pid, (&mut status as *mut i32) as u64, options, 0);
    if (res as i64) < 0 {
        return res;
    }

    if infop_ptr != 0 {
        let mut info = SigInfo::default();
        if res > 0 {
            info.si_signo = 17; // SIGCHLD
            info.si_code = 1;  // CLD_EXITED
            info.si_pid = res as u32;
            info.si_status = (status >> 8) & 0xff;
        }
        unsafe {
            core::ptr::write_unaligned(infop_ptr as *mut SigInfo, info);
        }
    }

    0
}

pub fn sys_clock_getres(_clock_id: u64, res_ptr: u64) -> u64 {
    if res_ptr != 0 {
        if !validate_user_buffer(res_ptr, 16) {
            return (-(EFAULT as i64)) as u64;
        }
        let res = crate::task::futex::Timespec {
            tv_sec: 0,
            tv_nsec: 10_000_000, // 10 мс (1 тик таймера PIT 100 Гц)
        };
        unsafe {
            core::ptr::write_unaligned(res_ptr as *mut crate::task::futex::Timespec, res);
        }
    }
    0
}

pub fn sys_clock_nanosleep(
    _clock_id: u64,
    flags: u64,
    req_ptr: u64,
    rem_ptr: u64,
) -> u64 {
    const TIMER_ABSTIME: u64 = 1;
    if !validate_user_buffer(req_ptr, 16) {
        return (-(EFAULT as i64)) as u64;
    }
    let req = unsafe { core::ptr::read_unaligned(req_ptr as *const crate::task::futex::Timespec) };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return (-(EINVAL as i64)) as u64;
    }

    if (flags & TIMER_ABSTIME) != 0 {
        let (cur_sec, cur_nsec) = get_realtime_epoch();
        if req.tv_sec < cur_sec || (req.tv_sec == cur_sec && req.tv_nsec <= cur_nsec) {
            return 0;
        }
        let delta_sec = (req.tv_sec - cur_sec) as u64;
        let delta_nsec = if req.tv_nsec >= cur_nsec {
            (req.tv_nsec - cur_nsec) as u64
        } else {
            1_000_000_000 - (cur_nsec - req.tv_nsec) as u64
        };
        let delta = delta_sec.saturating_mul(100) + (delta_nsec + 9_999_999) / 10_000_000;
        let deadline = crate::arch::x86_64::timer::ticks().saturating_add(delta);
        crate::task::block_current(Some(deadline));
    } else {
        return sys_nanosleep(req_ptr, rem_ptr);
    }

    if crate::task::signal::is_sigint_pending_for_current() {
        return (-(EINTR as i64)) as u64;
    }
    0
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
                let name_bytes = unsafe { core::slice::from_raw_parts(arg2 as *const u8, 16) };
                let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
                crate::task::set_current_thread_name(&name_bytes[..len]);
                0
            } else {
                (-(EFAULT as i64)) as u64
            }
        }
        PR_GET_NAME => {
            if arg2 != 0 && validate_user_buffer(arg2, 16) {
                let name = crate::task::current_thread_name();
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
        assert!(is_shell_binary_path("/bin/bash"));
        assert!(is_shell_binary_path("bash"));

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
