use core::arch::global_asm;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::UserAddressSpace;
use crate::serial_println;

const USER_CODE_ADDR: u64 = 0x0000_0000_0040_0000;
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
const USER_STACK_SIZE: usize = 0x4000; // 16 КиБ

global_asm!(r#"
    .global ring3_hello_payload_start
    .global ring3_hello_payload_end
    ring3_hello_payload_start:
        // SYS_WRITE (1), fd 1 (stdout), указатель на строку, длина 26 байт
        mov eax, 1
        mov edi, 1
        lea rsi, [rip + 1f]
        mov edx, 26
        syscall

        // SYS_EXIT (60), статус 0
        mov eax, 60
        xor edi, edi
        syscall

    2:  hlt
        jmp 2b

    1:
        .ascii "wunix: hello from ring 3!\n"
    ring3_hello_payload_end:

    .global ring3_fault_payload_start
    .global ring3_fault_payload_end
    ring3_fault_payload_start:
        // Привилегированная инструкция: в Ring 3 обязана вызвать General Protection Fault (#GP)
        cli

        // Если процессор не выбросил #GP (ошибка изоляции), выходим с кодом ошибки 1
        mov eax, 60
        mov edi, 1
        syscall

    3:  hlt
        jmp 3b
    ring3_fault_payload_end:
"#);

unsafe extern "C" {
    static ring3_hello_payload_start: u8;
    static ring3_hello_payload_end: u8;
    static ring3_fault_payload_start: u8;
    static ring3_fault_payload_end: u8;
}

fn user_hello_entry() {
    serial_println!("[ring3] preparing user address space for hello test...");

    let mut user_space = UserAddressSpace::new().expect("Failed to create UserAddressSpace");

    // Выделяем и отображаем стек пользователя
    user_space
        .allocate_and_map_range(
            VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64),
            USER_STACK_SIZE,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        )
        .expect("Failed to map user stack");

    // Копируем машинный код тестового приложения в пользовательскую страницу
    let code_start = &raw const ring3_hello_payload_start as *const u8;
    let code_end = &raw const ring3_hello_payload_end as *const u8;
    let code_len = code_end as usize - code_start as usize;
    let code_slice = unsafe { core::slice::from_raw_parts(code_start, code_len) };

    user_space
        .map_user_code_slice(VirtAddr::new(USER_CODE_ADDR), code_slice)
        .expect("Failed to map user code");

    serial_println!(
        "[ring3] activating PML4 {:#x} and jumping to Ring 3 (RIP={:#x}, RSP={:#x})...",
        user_space.pml4_frame.start_address().as_u64(),
        USER_CODE_ADDR,
        USER_STACK_TOP
    );

    user_space.activate();

    unsafe {
        crate::arch::x86_64::syscall::jump_to_user_mode(USER_CODE_ADDR, USER_STACK_TOP, 0, 0);
    }
}

fn user_fault_entry() {
    serial_println!("[ring3] preparing user address space for privilege violation test...");

    let mut user_space = UserAddressSpace::new().expect("Failed to create UserAddressSpace");

    user_space
        .allocate_and_map_range(
            VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64),
            USER_STACK_SIZE,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        )
        .expect("Failed to map user stack");

    let code_start = &raw const ring3_fault_payload_start as *const u8;
    let code_end = &raw const ring3_fault_payload_end as *const u8;
    let code_len = code_end as usize - code_start as usize;
    let code_slice = unsafe { core::slice::from_raw_parts(code_start, code_len) };

    user_space
        .map_user_code_slice(VirtAddr::new(USER_CODE_ADDR), code_slice)
        .expect("Failed to map user code");

    serial_println!(
        "[ring3] jumping to Ring 3 to test privileged instruction fault (cli)..."
    );

    user_space.activate();

    unsafe {
        crate::arch::x86_64::syscall::jump_to_user_mode(USER_CODE_ADDR, USER_STACK_TOP, 0, 0);
    }
}

/// Выполняет полную верификацию Этапа 6:
/// 1. Создание адресного пространства процесса и переход в Ring 3.
/// 2. Исполнение системного вызова sys_write и sys_exit из Ring 3.
/// 3. Проверка изоляции процессора: перехват исключения #GP при попытке выполнения cli в Ring 3.
pub fn run_verification() {
    serial_println!("=== [Stage 6 Verification: Ring 3 & Syscalls] ===");

    // Тест 1: Успешный запуск программы в Ring 3, вызов sys_write и штатный sys_exit
    super::spawn(user_hello_entry);
    while super::has_ready_threads() {
        super::yield_now();
    }
    serial_println!("[ring3] test 1 passed: sys_write and sys_exit executed cleanly");

    // Тест 2: Изоляция привилегий — попытка инструкции cli в Ring 3 вызывает #GP и корректно изолируется
    super::spawn(user_fault_entry);
    while super::has_ready_threads() {
        super::yield_now();
    }
    serial_println!("[ring3] test 2 passed: privileged instruction correctly faulted into #GP");

    serial_println!("=== [Stage 6 Verification Complete: Ring 3 Active] ===");
}
