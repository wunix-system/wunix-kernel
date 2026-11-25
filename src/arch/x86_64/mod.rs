pub mod apic;
pub mod cpu;
pub mod gdt;
pub mod interrupts;
pub mod syscall;
pub mod timer;

pub use cpu::CpuInfo;
pub use syscall::SyscallRegisters;

pub fn init() -> gdt::Selectors {
    use crate::i18n::t;

    cpu::enable_sse();
    crate::kboot!(
        "cpu0",
        "{}",
        t(
            "поддержка x87 FPU, SSE и FXSAVE включена",
            "x87 FPU, SSE and FXSAVE extensions enabled"
        )
    );

    let selectors = gdt::init();
    crate::kboot!(
        "gdt0",
        "{}",
        t(
            "таблица дескрипторов GDT и стеки исключений TSS/IST",
            "GDT descriptors and TSS/IST exception stacks initialized"
        )
    );

    interrupts::init_idt();
    crate::kboot!(
        "idt0",
        "{}",
        t(
            "таблица векторов прерываний (256 векторов)",
            "interrupt descriptor table (256 vectors)"
        )
    );

    unsafe {
        let mut pics = interrupts::PICS.lock();
        pics.initialize();
        // Разрешаем прерывания IRQ0 (таймер PIT) и IRQ1 (PS/2 клавиатура) на ведущем контроллере PIC1.
        // Бит 0 = IRQ0 (0 = разрешено), Бит 1 = IRQ1 (0 = разрешено).
        // Ведомый контроллер PIC2 полностью маскируем (0xFF).
        pics.write_masks(0xFC, 0xFF);
    };
    crate::kboot!(
        "pic0",
        "{}",
        t(
            "контроллер 8259 PIC (векторы 32..47, активны IRQ0, IRQ1)",
            "8259 PIC controller (vectors 32..47, IRQ0/IRQ1 active)"
        )
    );

    // Настраиваем быстрые системные вызовы: MSR-регистры STAR, LSTAR, FMASK, EFER.SCE
    syscall::init();
    syscall::set_kernel_stack(gdt::get_kernel_syscall_stack_top());
    crate::kboot!(
        "syscall0",
        "{}",
        t(
            "быстрые системные вызовы (MSR STAR/LSTAR/FMASK)",
            "fast system calls configured (MSR STAR/LSTAR/FMASK)"
        )
    );

    x86_64::instructions::interrupts::enable();
    crate::kboot!(
        "irq",
        "{}",
        t(
            "аппаратные прерывания процессора разрешены",
            "processor hardware interrupts enabled"
        )
    );

    selectors
}
