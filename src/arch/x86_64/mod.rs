pub mod cpu;
pub mod gdt;
pub mod interrupts;
pub mod syscall;
pub mod timer;

pub use cpu::CpuInfo;
pub use syscall::SyscallRegisters;

pub fn init() -> gdt::Selectors {
    cpu::enable_sse();
    let selectors = gdt::init();
    interrupts::init_idt();
    unsafe {
        let mut pics = interrupts::PICS.lock();
        pics.initialize();
        // Разрешаем прерывания IRQ0 (таймер PIT) и IRQ1 (PS/2 клавиатура) на ведущем контроллере PIC1.
        // Бит 0 = IRQ0 (0 = разрешено), Бит 1 = IRQ1 (0 = разрешено).
        // Ведомый контроллер PIC2 полностью маскируем (0xFF).
        pics.write_masks(0xFC, 0xFF);
    };

    // Настраиваем быстрые системные вызовы: MSR-регистры STAR, LSTAR, FMASK, EFER.SCE
    syscall::init();
    syscall::set_kernel_stack(gdt::get_kernel_syscall_stack_top());

    x86_64::instructions::interrupts::enable();
    selectors
}
