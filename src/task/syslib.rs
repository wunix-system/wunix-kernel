use core::sync::atomic::{AtomicU64, Ordering};
use spin::Once;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::memory::UserAddressSpace;

pub const SYSLIB_INFO_VADDR: u64 = 0x0000_7FFF_FFFF_0000;
pub const SYSLIB_CODE_VADDR: u64 = 0x0000_7FFF_FFFF_1000;
pub const AT_SYSINFO_EHDR: u64 = 33;

pub const SYSLIB_MAGIC: [u8; 8] = *b"WUNIXLIB";
pub const MAX_SYSLIB_SYMBOLS: usize = 40;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SyslibSymbol {
    pub name: [u8; 32],
    pub vaddr: u64,
    pub flags: u32,
    pub reserved: u32,
}

impl SyslibSymbol {
    pub const fn empty() -> Self {
        Self {
            name: [0; 32],
            vaddr: 0,
            flags: 0,
            reserved: 0,
        }
    }

    pub fn new(name_str: &str, vaddr: u64, flags: u32) -> Self {
        let mut name = [0u8; 32];
        let bytes = name_str.as_bytes();
        let len = bytes.len().min(31);
        let mut i = 0;
        while i < len {
            name[i] = bytes[i];
            i += 1;
        }
        Self {
            name,
            vaddr,
            flags,
            reserved: 0,
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        let q_bytes = query.as_bytes();
        if q_bytes.len() > 32 {
            return false;
        }
        let len = self.name_len();
        if len != q_bytes.len() {
            return false;
        }
        &self.name[..len] == q_bytes
    }

    pub fn name_len(&self) -> usize {
        let mut len = 0;
        while len < 32 && self.name[len] != 0 {
            len += 1;
        }
        len
    }
}

#[repr(C)]
pub struct SyslibSharedInfo {
    pub magic: [u8; 8],
    pub version: u32,
    pub num_symbols: u32,
    pub page_size: u32,
    pub ticks_per_sec: u32,
    pub boot_epoch_sec: u64,
    pub reserved: [u64; 2],
    pub symbols: [SyslibSymbol; MAX_SYSLIB_SYMBOLS],
}

static SYSLIB_INFO_FRAME: Once<PhysFrame<Size4KiB>> = Once::new();
static SYSLIB_CODE_FRAME: Once<PhysFrame<Size4KiB>> = Once::new();
static SYSLIB_SYMBOLS_COUNT: AtomicU64 = AtomicU64::new(0);

// Локальный кэш символов для быстрого in-kernel резолвинга PE/ELF импортов
static mut IN_KERNEL_SYMBOLS: [SyslibSymbol; MAX_SYSLIB_SYMBOLS] =
    [SyslibSymbol::empty(); MAX_SYSLIB_SYMBOLS];

struct SyslibBuilder {
    code_ptr: *mut u8,
    code_offset: usize,
    symbols: [SyslibSymbol; MAX_SYSLIB_SYMBOLS],
    sym_count: usize,
}

impl SyslibBuilder {
    fn new(code_ptr: *mut u8) -> Self {
        Self {
            code_ptr,
            code_offset: 0,
            symbols: [SyslibSymbol::empty(); MAX_SYSLIB_SYMBOLS],
            sym_count: 0,
        }
    }

    fn add_raw_func(&mut self, name: &str, bytes: &[u8], flags: u32) {
        if self.sym_count >= MAX_SYSLIB_SYMBOLS || self.code_offset + bytes.len() > 4096 {
            return;
        }
        let fn_vaddr = SYSLIB_CODE_VADDR + self.code_offset as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.code_ptr.add(self.code_offset), bytes.len());
        }
        self.symbols[self.sym_count] = SyslibSymbol::new(name, fn_vaddr, flags);
        self.sym_count += 1;
        // Выравниваем следующий вход по 8 байт
        self.code_offset = (self.code_offset + bytes.len() + 7) & !7;
    }

    fn add_linux_trampoline(&mut self, name: &str, nr: u32) {
        let mut code = [0u8; 8];
        code[0] = 0xb8;
        code[1..5].copy_from_slice(&nr.to_le_bytes());
        code[5] = 0x0f;
        code[6] = 0x05;
        code[7] = 0xc3;
        self.add_raw_func(name, &code, 1);
    }

    fn add_nt_trampoline(&mut self, name: &str, ssn: u32) {
        let mut code = [0u8; 11];
        code[0] = 0x49;
        code[1] = 0x89;
        code[2] = 0xca; // mov r10, rcx
        code[3] = 0xb8;
        code[4..8].copy_from_slice(&ssn.to_le_bytes()); // mov eax, ssn
        code[8] = 0x0f;
        code[9] = 0x05; // syscall
        code[10] = 0xc3; // ret
        self.add_raw_func(name, &code, 2);
    }
}

pub fn init_syslib() {
    let hhdm = crate::memory::hhdm_offset();

    let info_frame = SYSLIB_INFO_FRAME.call_once(|| {
        let frame = crate::memory::allocate_frame().expect("Syslib info frame allocation failed");
        let virt = VirtAddr::new(hhdm + frame.start_address().as_u64());
        unsafe {
            core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096);
        }
        frame
    });

    let code_frame = SYSLIB_CODE_FRAME.call_once(|| {
        let frame = crate::memory::allocate_frame().expect("Syslib code frame allocation failed");
        let virt = VirtAddr::new(hhdm + frame.start_address().as_u64());
        unsafe {
            core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096);
        }
        frame
    });

    let code_ptr = (hhdm + code_frame.start_address().as_u64()) as *mut u8;
    let mut builder = SyslibBuilder::new(code_ptr);

    // 1. Быстрые строковые функции (pure Ring 3 machine code)
    // strlen: mov rax, rdi; 1: cmp byte [rax], 0; je 2f; inc rax; jmp 1b; 2: sub rax, rdi; ret
    let strlen_code: [u8; 17] = [
        0x48, 0x89, 0xf8, 0x80, 0x38, 0x00, 0x74, 0x06, 0x48, 0xff, 0xc0, 0xeb, 0xf6, 0x48, 0x29,
        0xf8, 0xc3,
    ];
    builder.add_raw_func("strlen", &strlen_code, 0);

    // memcpy: mov rax, rdi; mov rcx, rdx; rep movsb; ret
    let memcpy_code: [u8; 9] = [0x48, 0x89, 0xf8, 0x48, 0x89, 0xd1, 0xf3, 0xa4, 0xc3];
    builder.add_raw_func("memcpy", &memcpy_code, 0);

    // memset: mov r8, rdi; mov al, sil; mov rcx, rdx; rep stosb; mov rax, r8; ret
    let memset_code: [u8; 15] = [
        0x49, 0x89, 0xf8, 0x40, 0x88, 0xf0, 0x48, 0x89, 0xd1, 0xf3, 0xaa, 0x4c, 0x89, 0xc0, 0xc3,
    ];
    builder.add_raw_func("memset", &memset_code, 0);

    // 2. Linux Syscall Trampolines
    builder.add_linux_trampoline("sys_read", 0);
    builder.add_linux_trampoline("sys_write", 1);
    builder.add_linux_trampoline("sys_close", 3);
    builder.add_linux_trampoline("sys_getpid", 39);
    builder.add_linux_trampoline("sys_exit", 60);

    // 3. Windows NT Syscall Trampolines
    builder.add_nt_trampoline("NtClose", 0x0006);
    builder.add_nt_trampoline("NtReadFile", 0x0013);
    builder.add_nt_trampoline("NtWriteFile", 0x0014);
    builder.add_nt_trampoline("NtAllocateVirtualMemory", 0x001C);
    builder.add_nt_trampoline("NtFreeVirtualMemory", 0x001D);
    builder.add_nt_trampoline("NtTerminateProcess", 0x0072);
    builder.add_nt_trampoline("NtDelayExecution", 0x0004);
    builder.add_nt_trampoline("NtYieldExecution", 0x0008);
    builder.add_nt_trampoline("NtCreateKey", 0x0050);
    builder.add_nt_trampoline("NtOpenKey", 0x0055);
    builder.add_nt_trampoline("NtQueryValueKey", 0x0057);
    builder.add_nt_trampoline("NtSetValueKey", 0x0058);
    builder.add_nt_trampoline("NtEnumerateKey", 0x005F);
    builder.add_nt_trampoline("NtDeleteValueKey", 0x006E);
    builder.add_nt_trampoline("NtCreateFile", 0x0012);
    builder.add_nt_trampoline("NtOpenFile", 0x000A);
    builder.add_nt_trampoline("NtQueryInformationFile", 0x000B);
    builder.add_nt_trampoline("NtQueryInformationProcess", 0x0007);
    builder.add_nt_trampoline("NtQuerySystemInformation", 0x0028);
    builder.add_nt_trampoline("NtProtectVirtualMemory", 0x0050);
    builder.add_nt_trampoline("NtDuplicateObject", 0x003C);

    // 4. Win32 Kernel32 wrappers
    // ExitProcess: mov r10, rcx; mov eax, 0x72 (NT_TERMINATE_PROCESS); syscall; ret
    let exit_process_code: [u8; 11] = [
        0x49, 0x89, 0xca, // mov r10, rcx
        0xb8, 0x72, 0x00, 0x00, 0x00, // mov eax, 0x72
        0x0f, 0x05, // syscall
        0xc3, // ret
    ];
    builder.add_raw_func("ExitProcess", &exit_process_code, 2);

    // GetStdHandle(nStdHandle):
    // rcx == -10 -> 0 (STDIN), rcx == -11 -> 1 (STDOUT), rcx == -12 -> 2 (STDERR), else -1
    let get_std_handle_code: [u8; 38] = [
        0x83, 0xf9, 0xf6, // cmp ecx, -10
        0x74, 0x16, // je stdin
        0x83, 0xf9, 0xf5, // cmp ecx, -11
        0x74, 0x14, // je stdout
        0x83, 0xf9, 0xf4, // cmp ecx, -12
        0x74, 0x12, // je stderr
        0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov rax, -1
        0xc3, // ret
        0x31, 0xc0, 0xc3, // stdin: xor eax, eax; ret
        0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3, // stdout: mov eax, 1; ret
        0xb8, 0x02, 0x00, 0x00, 0x00, 0xc3, // stderr: mov eax, 2; ret
    ];
    builder.add_raw_func("GetStdHandle", &get_std_handle_code, 2);

    // CloseHandle -> переход на NtClose
    let nt_close_vaddr = builder.symbols[..builder.sym_count]
        .iter()
        .find(|s| s.matches("NtClose"))
        .map(|s| s.vaddr);

    if let Some(vaddr) = nt_close_vaddr {
        let jmp_offset = builder.code_offset;
        builder.add_raw_func("CloseHandle", &[0xe9, 0, 0, 0, 0], 2);
        let rel32 = (vaddr as i64 - (SYSLIB_CODE_VADDR as i64 + jmp_offset as i64 + 5)) as i32;
        unsafe {
            core::ptr::copy_nonoverlapping(
                rel32.to_le_bytes().as_ptr(),
                code_ptr.add(jmp_offset + 1),
                4,
            );
        }
    }

    let final_symbols = builder.symbols;
    let final_count = builder.sym_count;

    // Сохраняем символы в статический кэш
    unsafe {
        IN_KERNEL_SYMBOLS = final_symbols;
    }
    SYSLIB_SYMBOLS_COUNT.store(final_count as u64, Ordering::Relaxed);

    // Записываем заголовок SyslibSharedInfo в страницу INFO
    let info_ptr = (hhdm + info_frame.start_address().as_u64()) as *mut SyslibSharedInfo;
    let dt = crate::drivers::rtc::read_datetime();
    let boot_time = crate::drivers::rtc::rtc_to_epoch(&dt) as u64;
    unsafe {
        (*info_ptr).magic = SYSLIB_MAGIC;
        (*info_ptr).version = 1;
        (*info_ptr).num_symbols = final_count as u32;
        (*info_ptr).page_size = 4096;
        (*info_ptr).ticks_per_sec = 100;
        (*info_ptr).boot_epoch_sec = boot_time;
        (*info_ptr).reserved = [0, 0];
        (*info_ptr).symbols = final_symbols;
    }
}

pub fn map_syslib_to_user_space(user_space: &mut UserAddressSpace) -> Result<(), &'static str> {
    let info_frame = SYSLIB_INFO_FRAME.get().ok_or("Syslib not initialized")?;
    let code_frame = SYSLIB_CODE_FRAME.get().ok_or("Syslib not initialized")?;

    let info_page = Page::<Size4KiB>::containing_address(VirtAddr::new(SYSLIB_INFO_VADDR));
    let code_page = Page::<Size4KiB>::containing_address(VirtAddr::new(SYSLIB_CODE_VADDR));

    // Страница INFO: Present + User (Read Only)
    user_space.map_page(
        info_page,
        *info_frame,
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE,
    )?;

    // Страница CODE: Present + User (Read + Execute)
    user_space.map_page(
        code_page,
        *code_frame,
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE,
    )?;

    Ok(())
}

pub fn resolve_symbol_vaddr(name: &str) -> Option<u64> {
    let count = SYSLIB_SYMBOLS_COUNT.load(Ordering::Relaxed) as usize;
    unsafe {
        for i in 0..count {
            if IN_KERNEL_SYMBOLS[i].matches(name) {
                return Some(IN_KERNEL_SYMBOLS[i].vaddr);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_syslib_symbol_matching() {
        let sym = SyslibSymbol::new("NtClose", 0x7fff_ffff_1000, 2);
        assert!(sym.matches("NtClose"));
        assert!(!sym.matches("NtReadFile"));
        assert!(!sym.matches("Close"));
    }
}
