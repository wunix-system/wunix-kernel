pub mod file;
pub mod registry;

use crate::arch::x86_64::SyscallRegisters;
use crate::serial_println;
pub use file::*;
pub use registry::*;

// Системные номера системных вызовов Windows NT x64 (SSN - System Service Numbers)
pub const NT_DEVICE_IO_CONTROL_FILE: u64 = 0x0002;
pub const NT_WAIT_FOR_SINGLE_OBJECT: u64 = 0x0003;
pub const NT_DELAY_EXECUTION: u64 = 0x0004;
pub const NT_CLOSE: u64 = 0x0006;
pub const NT_QUERY_INFORMATION_PROCESS: u64 = 0x0007;
pub const NT_YIELD_EXECUTION: u64 = 0x0008;
pub const NT_OPEN_FILE: u64 = 0x000A;
pub const NT_QUERY_INFORMATION_FILE: u64 = 0x000B;
pub const NT_CREATE_FILE: u64 = 0x0012;
pub const NT_READ_FILE: u64 = 0x0013;
pub const NT_WRITE_FILE: u64 = 0x0014;
pub const NT_ALLOCATE_VIRTUAL_MEMORY: u64 = 0x001C;
pub const NT_FREE_VIRTUAL_MEMORY: u64 = 0x001D;
pub const NT_QUERY_SYSTEM_INFORMATION: u64 = 0x0028;
pub const NT_DUPLICATE_OBJECT: u64 = 0x003C;
pub const NT_CREATE_THREAD: u64 = 0x0041;
pub const NT_PROTECT_VIRTUAL_MEMORY: u64 = 0x004D;
pub const NT_CREATE_KEY: u64 = 0x0050;
pub const NT_OPEN_KEY: u64 = 0x0055;
pub const NT_QUERY_VALUE_KEY: u64 = 0x0057;
pub const NT_SET_VALUE_KEY: u64 = 0x0058;
pub const NT_ENUMERATE_KEY: u64 = 0x005F;
pub const NT_DELETE_VALUE_KEY: u64 = 0x006E;
pub const NT_TERMINATE_PROCESS: u64 = 0x0072;
pub const NT_CREATE_EVENT: u64 = 0x007A;

/// Диспетчер нативных системных вызовов Windows NT API ядра Wunix
pub fn win_syscall_dispatcher(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    regs: &SyscallRegisters,
) -> u64 {
    match nr {
        NT_CLOSE => {
            if nt_close_reg_handle(arg1) {
                STATUS_SUCCESS
            } else {
                let res = crate::task::syscall::sys_close(arg1);
                if res == 0 {
                    STATUS_SUCCESS
                } else {
                    STATUS_INVALID_PARAMETER
                }
            }
        }
        NT_YIELD_EXECUTION => {
            crate::task::yield_now();
            STATUS_SUCCESS
        }
        NT_DELAY_EXECUTION => {
            // arg2: указатель на LARGE_INTEGER (отрицательное значение в сотнях наносекунд)
            if arg2 != 0 {
                let interval = unsafe { *(arg2 as *const i64) };
                if interval < 0 {
                    let nanos = (-interval as u64).saturating_mul(100);
                    let sec = nanos / 1_000_000_000;
                    let nsec = nanos % 1_000_000_000;
                    let ts = crate::task::futex::Timespec {
                        tv_sec: sec as i64,
                        tv_nsec: nsec as i64,
                    };
                    let delta = (ts.tv_sec as u64).saturating_mul(100) + (ts.tv_nsec as u64 + 9_999_999) / 10_000_000;
                    let deadline = crate::arch::x86_64::timer::ticks().saturating_add(delta);
                    crate::task::block_current(Some(deadline));
                }
            }
            STATUS_SUCCESS
        }
        NT_WAIT_FOR_SINGLE_OBJECT => {
            // Handle, Alertable, Timeout
            if arg3 != 0 {
                let interval = unsafe { *(arg3 as *const i64) };
                if interval < 0 {
                    let ticks = ((-interval as u64) / 100_000).max(1);
                    let deadline = crate::arch::x86_64::timer::ticks().saturating_add(ticks);
                    crate::task::block_current(Some(deadline));
                }
            } else {
                crate::task::yield_now();
            }
            STATUS_SUCCESS
        }
        NT_READ_FILE => {
            // NtReadFile(FileHandle, Event, ApcRoutine, ApcContext, IoStatusBlock, Buffer, Length, ...)
            // В Windows x64 System V: arg1=Handle, arg2=Event, arg3=Apc, arg4=ApcCtx, arg5=IoStatusBlock, r9=Buffer
            let buf_ptr = regs.r9;
            let len = unsafe { *((regs.user_rsp + 40) as *const u32) } as u64; // аргумент на стеке
            let bytes_read = crate::task::syscall::sys_read(arg1, buf_ptr, len);
            if (bytes_read as i64) < 0 {
                STATUS_UNSUCCESSFUL
            } else {
                if arg5 != 0 {
                    unsafe {
                        // IoStatusBlock: [Status: 8 bytes][Information: 8 bytes]
                        core::ptr::write_unaligned(arg5 as *mut u64, STATUS_SUCCESS);
                        core::ptr::write_unaligned((arg5 + 8) as *mut u64, bytes_read);
                    }
                }
                STATUS_SUCCESS
            }
        }
        NT_WRITE_FILE => {
            let buf_ptr = regs.r9;
            let len = unsafe { *((regs.user_rsp + 40) as *const u32) } as u64;
            let bytes_written = crate::task::syscall::sys_write(arg1, buf_ptr, len);
            if (bytes_written as i64) < 0 {
                STATUS_UNSUCCESSFUL
            } else {
                if arg5 != 0 {
                    unsafe {
                        core::ptr::write_unaligned(arg5 as *mut u64, STATUS_SUCCESS);
                        core::ptr::write_unaligned((arg5 + 8) as *mut u64, bytes_written);
                    }
                }
                STATUS_SUCCESS
            }
        }
        NT_ALLOCATE_VIRTUAL_MEMORY => {
            // NtAllocateVirtualMemory(ProcessHandle, BaseAddressPtr, ZeroBits, RegionSizePtr, AllocationType, Protect)
            if arg2 == 0 || arg4 == 0 {
                return STATUS_INVALID_PARAMETER;
            }
            let req_addr = unsafe { *(arg2 as *const u64) };
            let req_size = unsafe { *(arg4 as *const u64) };
            let prot = 0x3; // PROT_READ | PROT_WRITE
            let flags = 0x22; // MAP_PRIVATE | MAP_ANONYMOUS
            let allocated = crate::task::syscall::sys_mmap(req_addr, req_size, prot, flags, 0, 0);
            if (allocated as i64) < 0 {
                STATUS_UNSUCCESSFUL
            } else {
                unsafe {
                    core::ptr::write_unaligned(arg2 as *mut u64, allocated);
                }
                STATUS_SUCCESS
            }
        }
        NT_FREE_VIRTUAL_MEMORY => {
            if arg2 == 0 || arg3 == 0 {
                return STATUS_INVALID_PARAMETER;
            }
            let base_addr = unsafe { *(arg2 as *const u64) };
            let region_size = unsafe { *(arg3 as *const u64) };
            let res = crate::task::syscall::sys_munmap(base_addr, region_size);
            if res == 0 {
                STATUS_SUCCESS
            } else {
                STATUS_UNSUCCESSFUL
            }
        }
        NT_CREATE_FILE => {
            nt_create_file(arg1, arg2, arg3, arg4, arg5, regs.r9, 0, 1, 0, 0, 0)
        }
        NT_OPEN_FILE => {
            nt_open_file(arg1, arg2, arg3, arg4, arg5, regs.r9)
        }
        NT_QUERY_INFORMATION_FILE => {
            nt_query_information_file(arg1, arg2, arg3, arg4, arg5)
        }
        NT_QUERY_INFORMATION_PROCESS => {
            nt_query_information_process(arg1, arg2, arg3, arg4, arg5)
        }
        NT_QUERY_SYSTEM_INFORMATION => {
            nt_query_system_information(arg1, arg2, arg3, arg4)
        }
        NT_DUPLICATE_OBJECT => {
            let res = crate::task::syscall::sys_dup(arg1);
            if (res as i64) >= 0 {
                if arg2 != 0 {
                    unsafe {
                        core::ptr::write_unaligned(arg2 as *mut u64, res);
                    }
                }
                STATUS_SUCCESS
            } else {
                STATUS_UNSUCCESSFUL
            }
        }
        NT_PROTECT_VIRTUAL_MEMORY => {
            if arg2 == 0 || arg3 == 0 {
                return STATUS_INVALID_PARAMETER;
            }
            let base_addr = unsafe { *(arg2 as *const u64) };
            let region_size = unsafe { *(arg3 as *const u64) };
            let prot = arg4;
            let res = crate::task::syscall::sys_mprotect(base_addr, region_size, prot);
            if res == 0 {
                STATUS_SUCCESS
            } else {
                STATUS_UNSUCCESSFUL
            }
        }
        NT_OPEN_KEY => nt_open_key(arg1, arg2, arg3),
        NT_CREATE_KEY => nt_create_key(arg1, arg2, arg3, arg4, arg5, regs.r9, 0),
        NT_QUERY_VALUE_KEY => {
            nt_query_value_key(arg1, arg2, arg3, arg4, arg5, regs.r9)
        }
        NT_SET_VALUE_KEY => {
            nt_set_value_key(arg1, arg2, arg3, arg4, arg5, regs.r9)
        }
        NT_ENUMERATE_KEY => {
            nt_enumerate_key(arg1, arg2, arg3, arg4, arg5, regs.r9)
        }
        NT_DELETE_VALUE_KEY => {
            nt_delete_value_key(arg1, arg2)
        }
        NT_TERMINATE_PROCESS => {
            let code = if arg2 != 0 { arg2 } else { arg1 };
            crate::task::syscall::sys_exit_group(code);
        }
        NT_CREATE_EVENT => {
            let fd = crate::task::syscall::sys_eventfd2(arg5 as u32, 0);
            if (fd as i64) < 0 {
                STATUS_UNSUCCESSFUL
            } else {
                if arg1 != 0 {
                    unsafe {
                        core::ptr::write_unaligned(arg1 as *mut u64, fd);
                    }
                }
                STATUS_SUCCESS
            }
        }
        _ => {
            serial_println!("[nt-syscall] unhandled NT call SSN={:#x}", nr);
            STATUS_UNSUCCESSFUL
        }
    }
}
