use core::arch::global_asm;

// MSR-адреса для настройки быстрых системных вызовов x86_64
pub const IA32_EFER: u32 = 0xC000_0080;
pub const IA32_STAR: u32 = 0xC000_0081;
pub const IA32_LSTAR: u32 = 0xC000_0082;
pub const IA32_FMASK: u32 = 0xC000_0084;
pub const IA32_FS_BASE: u32 = 0xC000_0100;

const EFER_SCE: u64 = 1 << 0;

// Маска RFLAGS при входе в syscall: гасим IF (прерывания), DF (направление), TF (трассировка).
// Без маскирования IF первый же тик таймера PIT посреди syscall_entry (до сохранения контекста)
// уронит стек, потому что RSP ещё указывает на пользовательское пространство.
const FMASK_VALUE: u64 = 0x200 | 0x400 | 0x100; // IF | DF | TF

pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    }
    (high as u64) << 32 | low as u64
}

pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem, nostack));
    }
}

#[inline]
pub unsafe fn read_fs_base() -> u64 {
    unsafe { rdmsr(IA32_FS_BASE) }
}

#[inline]
pub unsafe fn write_fs_base(base: u64) {
    unsafe { wrmsr(IA32_FS_BASE, base); }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SyscallRegisters {
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub user_rip: u64,
    pub user_rflags: u64,
    pub user_rsp: u64,
}

unsafe extern "C" {
    fn syscall_entry();
}

/// Настраивает MSR-регистры для поддержки инструкций syscall/sysret в Long Mode.
///
/// Вызывать строго после инициализации GDT (дескрипторы Ring 0/3 должны быть на местах).
pub fn init() {
    unsafe {
        // Включаем бит SCE в EFER — без него процессор бросит #UD на инструкцию syscall
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | EFER_SCE);

        // STAR: kernel CS base в [47:32], user segment base в [63:48].
        // При syscall: CPU загружает CS = [47:32] = 0x08, SS = [47:32]+8 = 0x10.
        // При sysretq: CPU загружает CS = [63:48]+16 = 0x20 (|RPL=3 → 0x23),
        //              SS = [63:48]+8 = 0x18 (|RPL=3 → 0x1B).
        let star: u64 = (0x10u64 << 48) | (0x08u64 << 32);
        wrmsr(IA32_STAR, star);

        // LSTAR — адрес точки входа обработчика syscall в ядре
        wrmsr(IA32_LSTAR, syscall_entry as *const () as u64);

        // FMASK — биты RFLAGS, которые процессор обнулит при входе в syscall
        wrmsr(IA32_FMASK, FMASK_VALUE);
    }

    crate::serial_println!("syscall: MSR configured (STAR={:#x}, LSTAR={:#x})",
        unsafe { rdmsr(IA32_STAR) },
        unsafe { rdmsr(IA32_LSTAR) });
}

// Точка входа syscall — ассемблерный трамплин.
//
// При выполнении инструкции syscall процессор:
//   RCX ← user RIP,  R11 ← user RFLAGS
//   CS ← STAR[47:32], SS ← STAR[47:32]+8
//   RIP ← LSTAR
//   RFLAGS &= ~FMASK
//   RSP — НЕ ТРОГАЕТ! Остаётся пользовательский.
//
// Поэтому первым делом сохраняем user RSP и загружаем стек ядра.
global_asm!(r#"
    .global syscall_entry
    .extern syscall_dispatcher

    syscall_entry:
        // RSP пока пользовательский — сохраняем в scratch-буфер
        mov [rip + user_rsp_scratch], rsp
        mov rsp, [rip + kernel_rsp_scratch]

        // Теперь на стеке ядра — строим фрейм для восстановления при sysretq
        push [rip + user_rsp_scratch]   // user RSP
        push r11                        // user RFLAGS (сохранён процессором в R11)
        push rcx                        // user RIP    (сохранён процессором в RCX)

        // Сохраняем пользовательские регистры (callee-saved + caller-saved)
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        push rdi
        push rsi
        push rdx
        push r8
        push r9
        push r10

        // Перетасовка аргументов из syscall ABI в System V AMD64 calling convention:
        //   syscall ABI:  RAX=nr, RDI=a1, RSI=a2, RDX=a3, R10=a4, R8=a5, R9=a6
        //   C ABI:        RDI=nr, RSI=a1, RDX=a2, RCX=a3, R8=a4,  R9=a5
        //   7-й аргумент: указатель на сохраненные SyscallRegisters (передается на стеке)
        mov r11, rax        // Сохраняем nr в r11
        mov rax, rsp        // rax = &SyscallRegisters
        push rax            // 7-й аргумент на стеке (выравнивание 16 байт соблюдено)

        mov r9, [rax + 16]  // C arg5 = a5 (из R8)
        mov r8, [rax + 0]   // C arg4 = a4 (из R10)
        mov rcx, [rax + 24] // C arg3 = a3 (из RDX)
        mov rdx, [rax + 32] // C arg2 = a2 (из RSI)
        mov rsi, [rax + 40] // C arg1 = a1 (из RDI)
        mov rdi, r11        // C arg0 = nr

        // Контекст полностью сохранён, стек ядра активен — можно включить прерывания
        sti

        call syscall_dispatcher

        // Выключаем прерывания перед восстановлением пользовательского контекста.
        cli

        add rsp, 8          // Освобождаем 7-й аргумент

        // RAX содержит возвращаемое значение

        pop r10
        pop r9
        pop r8
        pop rdx
        pop rsi
        pop rdi
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp

        pop rcx             // user RIP
        pop r11             // user RFLAGS
        pop rsp             // user RSP — восстанавливаем последним

        sysretq

    .global ret_from_fork
    ret_from_fork:
        // Точка входа для нового потока, созданного через SYS_CLONE.
        // context_switch снял r15..rbp и передал управление сюда через ret.
        // На стеке лежат: r9 (gp), rsi (child_user_rsp), rdi (0), user RIP, user RSP.
        mov ax, 0x18 | 3
        mov ds, ax
        mov es, ax

        pop r9              // Восстанавливаем r9 (указатель gp в рантайме Go)
        pop rsi             // Восстанавливаем rsi = child_user_rsp (рантайм Go загружает RSP из RSI)
        pop rdi             // Очищаем rdi
        pop rcx             // user RIP
        pop rsp             // user RSP
        xor edx, edx
        mov r11, 0x202      // user RFLAGS (IF=1)
        xor eax, eax        // Возвращаемое значение clone в дочернем потоке = 0
        sysretq

    .global enter_user_child
    enter_user_child:
        // RDI = entry_point, RSI = user_stack_top, RDX = pml4_phys, RCX = argc, R8 = argv, R9 = child_kernel_stack_top
        // Отключаем прерывания на время сохранения контекста и переключения CR3/стека
        cli

        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15

        mov rax, cr3
        push rax

        mov rax, [rip + kernel_rsp_scratch]
        push rax

        // Сохраняем указатель на вершину стека родителя в таблице
        mov r10, [rip + child_stack_depth]
        lea r11, [rip + child_saved_rsp_table]
        mov [r11 + r10 * 8], rsp
        inc r10
        mov [rip + child_stack_depth], r10

        // Переключаем RSP ядра на выделенный стек дочернего процесса (R9)
        mov rsp, r9
        mov [rip + kernel_rsp_scratch], r9

        // Переключаем CR3 на таблицу страниц процесса потомка
        mov cr3, rdx

        // Настраиваем стек для iretq: SS, RSP, RFLAGS, CS, RIP
        // iretq атомарно восстановит RFLAGS с флагом IF=1 (0x202), включив прерывания в Ring 3
        mov ax, 0x18 | 3
        mov ds, ax
        mov es, ax
        push rax
        push rsi        // user RSP
        push 0x202      // user RFLAGS (IF=1)
        push 0x20 | 3   // user CS (Ring 3)
        push rdi        // user RIP (entry_point)

        // Передаем аргументы argc в RDI и argv в RSI для System V AMD64 ABI
        mov rdi, rcx
        mov rsi, r8

        xor rax, rax
        xor rdx, rdx
        xor rcx, rcx
        xor r8, r8
        xor r9, r9
        xor r10, r10
        xor r11, r11

        iretq

    .global is_child_running
    is_child_running:
        // Дочерняя команда активна только на глубине > 1 (глубина 1 — сама оболочка wsh)
        mov rax, [rip + child_stack_depth]
        cmp rax, 1
        seta al
        movzx rax, al
        ret

    .global has_parent_process
    has_parent_process:
        // Проверяет, запущен ли процесс потомком (глубина >= 1)
        mov rax, [rip + child_stack_depth]
        test rax, rax
        setnz al
        movzx rax, al
        ret

    .global resume_parent_process
    resume_parent_process:
        // RDI = exit_status
        // Блокируем прерывания: переключение стека ядра и CR3 должно быть строго атомарным
        cli

        mov rcx, [rip + child_stack_depth]
        test rcx, rcx
        jz 1f

        dec rcx
        mov [rip + child_stack_depth], rcx
        lea r8, [rip + child_saved_rsp_table]
        mov rsp, [r8 + rcx * 8]

        pop rax
        mov [rip + kernel_rsp_scratch], rax

        pop rax
        mov cr3, rax

        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp

        sti
        mov rax, rdi
        ret

    1:
        // Защита: если таблица стеков пуста (PID 1 завершился), глушим процессор
        cli
        hlt
        jmp 1b
"#);

#[unsafe(no_mangle)]
static mut user_rsp_scratch: u64 = 0;

#[unsafe(no_mangle)]
static mut kernel_rsp_scratch: u64 = 0;

#[unsafe(no_mangle)]
static mut child_stack_depth: u64 = 0;

#[unsafe(no_mangle)]
static mut child_saved_rsp_table: [u64; 32] = [0; 32];

unsafe extern "C" {
    pub fn is_child_running() -> bool;
    pub fn has_parent_process() -> bool;
    pub fn enter_user_child(
        entry_point: u64,
        stack_top: u64,
        pml4_phys: u64,
        argc: u64,
        argv: u64,
        child_kernel_stack_top: u64,
    ) -> u64;
    pub fn resume_parent_process(exit_status: u64) -> !;
    pub fn ret_from_fork();
}

pub fn current_child_stack_depth() -> u64 {
    unsafe { child_stack_depth }
}

/// Устанавливает адрес вершины стека ядра для syscall_entry.
/// Ассемблерный трамплин загрузит этот адрес в RSP при входе из Ring 3.
pub fn set_kernel_stack(stack_top: u64) {
    unsafe {
        kernel_rsp_scratch = stack_top;
    }
}

/// Выполняет аппаратный переход из Ring 0 в Ring 3 с переключением на пользовательский стек
/// и передачей управления по адресу `entry_point`.
/// Передает аргументы командной строки `argc` в RDI и `argv` в RSI.
///
/// # Safety
/// Вызывающий код обязан убедиться, что:
/// 1. В CR3 загружена таблица страниц с отображенными страницами `entry_point` и `stack_top`.
/// 2. Сегменты GDT 0x18 (User Data) и 0x20 (User Code) настроены.
/// 3. В TSS настроен стек ядра `rsp0`.
pub unsafe fn jump_to_user_mode(entry_point: u64, stack_top: u64, argc: u64, argv: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "mov ds, ax",
            "mov es, ax",
            "push rax",         // SS (0x1B)
            "push rsi",         // RSP (stack_top)
            "push rdx",         // RFLAGS (0x202: IF=1)
            "push rcx",         // CS (0x23)
            "push r8",          // RIP (entry_point)
            "mov rdi, r9",      // argc
            "mov rsi, r10",     // argv
            "xor rax, rax",
            "xor rdx, rdx",
            "xor rcx, rcx",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            "iretq",
            in("rax") 0x18u64 | 3,
            in("rsi") stack_top,
            in("rdx") 0x202u64,
            in("rcx") 0x20u64 | 3,
            in("r8") entry_point,
            in("r9") argc,
            in("r10") argv,
            options(noreturn),
        );
    }
}
