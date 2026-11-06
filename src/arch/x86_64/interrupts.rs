use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;
use crate::serial_println;

// Ремапим векторы PIC на диапазон 32..47, чтобы не конфликтовать со штатными исключениями CPU
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub fn init_idt() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.simd_floating_point.set_handler_fn(simd_floating_point_handler);

        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);

        idt
    });
    idt.load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    let accessed_addr = Cr2::read();
    let is_user = (stack_frame.code_segment.0 & 3) == 3;
    let is_user_addr = accessed_addr.as_ref().map(|a| a.as_u64() < 0x0000_8000_0000_0000).unwrap_or(false);

    if is_user || is_user_addr {
        if let Ok(addr) = accessed_addr {
            if crate::memory::handle_user_page_fault(addr, error_code).is_ok() {
                return;
            }
        }
        if is_user {
            serial_println!(
                "[kernel] #PF in Ring 3 at {:?}, error: {:?}, RIP {:#x} - terminating user task",
                accessed_addr,
                error_code,
                stack_frame.instruction_pointer.as_u64()
            );
            crate::task::syscall::sys_exit(128 + 14);
        }
    }

    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("Accessed Address: {:?}", accessed_addr);
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    crate::halt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user = (stack_frame.code_segment.0 & 3) == 3;

    if is_user {
        serial_println!(
            "[kernel] #GP in Ring 3 (code: {:#x}) at RIP {:#x} - privileged instruction trapped",
            error_code,
            stack_frame.instruction_pointer.as_u64()
        );
        crate::task::syscall::sys_exit(128 + 11);
    } else {
        serial_println!("EXCEPTION: GENERAL PROTECTION FAULT (code: {:#x})", error_code);
        serial_println!("{:#?}", stack_frame);
        crate::halt_loop();
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    let is_user = (stack_frame.code_segment.0 & 3) == 3;
    if is_user {
        serial_println!(
            "[kernel] #UD (Invalid Opcode) in Ring 3 at RIP {:#x} - terminating user task",
            stack_frame.instruction_pointer.as_u64()
        );
        crate::task::syscall::sys_exit(128 + 4);
    } else {
        panic!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
    }
}

extern "x86-interrupt" fn simd_floating_point_handler(stack_frame: InterruptStackFrame) {
    let is_user = (stack_frame.code_segment.0 & 3) == 3;
    if is_user {
        serial_println!(
            "[kernel] #XF (SIMD Floating-Point Exception) in Ring 3 at RIP {:#x}",
            stack_frame.instruction_pointer.as_u64()
        );
        crate::task::syscall::sys_exit(128 + 8);
    } else {
        panic!("EXCEPTION: SIMD FLOATING POINT\n{:#?}", stack_frame);
    }
}

extern "x86-interrupt" fn timer_interrupt_handler(stack_frame: InterruptStackFrame) {
    super::timer::on_tick();
    crate::net::poll();

    // Опрашиваем последовательный порт COM1 на каждом тике таймера (10 мс).
    // Ограничиваем порцию до 64 байт за тик, чтобы прерывание не могло зациклиться.
    let mut rx_count = 0;
    while let Some(b) = crate::drivers::serial::try_receive() {
        if b == 0x03 {
            crate::task::signal::raise_sigint();
            if !crate::task::syscall::proc::is_current_interactive_shell() {
                crate::fs::handle::flush_stdin();
                crate::drivers::keyboard::clear_key_queue();
                continue;
            }
        }
        crate::fs::handle::push_stdin_byte(b);
        rx_count += 1;
        if rx_count >= 64 {
            break;
        }
    }

    unsafe {
        // EOI контроллеру PIC отправляем ДО переключения контекста, иначе PIC заблокирует последующие тики
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    let is_user = (stack_frame.code_segment.0 & 3) == 3;
    if is_user
        && crate::task::signal::is_sigint_pending()
        && !crate::task::syscall::proc::is_current_interactive_shell()
    {
        crate::task::signal::clear_sigint();
        crate::fs::handle::flush_stdin();
        crate::drivers::keyboard::clear_key_queue();
        crate::serial_println!("^C");
        crate::println!("^C");
        crate::task::syscall::sys_exit(130);
    }

    crate::task::on_timer_tick(is_user);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Считываем скан-код из порта контроллера клавиатуры i8042.
    // Чтение байта из 0x60 сбрасывает статус готовности контроллера и разрешает прием следующих клавиш.
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    crate::drivers::keyboard::handle_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}
