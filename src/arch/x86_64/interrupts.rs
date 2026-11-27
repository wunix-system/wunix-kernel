use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;
use crate::serial_println;

// Ремапим векторы IRQ на диапазон 32..47, чтобы не конфликтовать со штатными исключениями CPU.
// Векторы 48..63 зарезервированы под динамические PCI/MSI/UIO прерывания.
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;
pub const MSI_VECTOR_BASE: u8 = 48;
pub const MSI_VECTOR_END: u8 = 63;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    Mouse = PIC_1_OFFSET + 12,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

pub type IrqHandlerFn = fn(u8);

#[derive(Clone, Copy)]
struct RegisteredIrq {
    name: &'static str,
    handler: Option<IrqHandlerFn>,
}

const EMPTY_IRQ: RegisteredIrq = RegisteredIrq {
    name: "",
    handler: None,
};

static IRQ_COUNTS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static IRQ_HANDLERS: Mutex<[RegisteredIrq; 32]> = Mutex::new([EMPTY_IRQ; 32]);

#[inline]
fn record_irq(vector: u8) {
    if let Some(idx) = vector.checked_sub(PIC_1_OFFSET) {
        if (idx as usize) < IRQ_COUNTS.len() {
            IRQ_COUNTS[idx as usize].fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[inline]
fn end_of_interrupt(vector: u8) {
    if super::apic::is_active() {
        super::apic::lapic::eoi();
    } else {
        unsafe {
            PICS.lock().notify_end_of_interrupt(vector);
        }
    }
}

/// Разрешает линию ISA IRQ (0..15) либо через IO-APIC, либо через 8259 PIC.
pub fn unmask_isa_irq(irq: u8) {
    if irq >= 16 {
        return;
    }
    if super::apic::is_active() {
        super::apic::route_isa_irq(irq, PIC_1_OFFSET + irq);
    } else {
        unsafe {
            use x86_64::instructions::port::Port;
            if irq < 8 {
                let mut p = Port::<u8>::new(0x21);
                let mask = p.read() & !(1 << irq);
                p.write(mask);
            } else {
                let mut p1 = Port::<u8>::new(0x21);
                let v1 = p1.read() & !(1 << 2); // каскад PIC2
                p1.write(v1);
                let mut p2 = Port::<u8>::new(0xA1);
                let mask = p2.read() & !(1 << (irq - 8));
                p2.write(mask);
            }
        }
    }
}

/// Регистрирует обработчик для вектора `32..=63` (ISA IRQ или MSI).
pub fn register_irq_handler(vector: u8, name: &'static str, handler: IrqHandlerFn) -> bool {
    let Some(idx) = vector.checked_sub(PIC_1_OFFSET) else {
        return false;
    };
    if (idx as usize) >= 32 {
        return false;
    }
    let mut table = IRQ_HANDLERS.lock();
    table[idx as usize] = RegisteredIrq {
        name,
        handler: Some(handler),
    };
    true
}

/// Выделяет свободный MSI/динамический вектор из диапазона `48..=63` и привязывает обработчик.
pub fn allocate_msi_vector(name: &'static str, handler: IrqHandlerFn) -> Option<u8> {
    let mut table = IRQ_HANDLERS.lock();
    for vec in MSI_VECTOR_BASE..=MSI_VECTOR_END {
        let idx = (vec - PIC_1_OFFSET) as usize;
        if table[idx].handler.is_none() {
            table[idx] = RegisteredIrq {
                name,
                handler: Some(handler),
            };
            return Some(vec);
        }
    }
    None
}

/// Возвращает реальную статистику прерываний `(irq_num, name, count)` для `/proc/interrupts`.
pub fn irq_stats() -> Vec<(u8, &'static str, u64)> {
    let table = IRQ_HANDLERS.lock();
    let mut out = Vec::new();
    for idx in 0..32 {
        let count = IRQ_COUNTS[idx].load(Ordering::Relaxed);
        let default_name = match idx {
            0 => "timer0 (pit)",
            1 => "i8042 (keyboard)",
            12 => "i8042 (ps2-mouse)",
            _ => "",
        };
        let name = if !table[idx].name.is_empty() {
            table[idx].name
        } else {
            default_name
        };
        if !name.is_empty() || count > 0 {
            out.push((
                idx as u8,
                if name.is_empty() { "pci/msi" } else { name },
                count,
            ));
        }
    }
    out
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
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.simd_floating_point
            .set_handler_fn(simd_floating_point_handler);

        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_u8()].set_handler_fn(mouse_interrupt_handler);

        // Векторы 48..=63 для динамических PCI/MSI и UIO прерываний
        idt[48].set_handler_fn(msi_handler_48);
        idt[49].set_handler_fn(msi_handler_49);
        idt[50].set_handler_fn(msi_handler_50);
        idt[51].set_handler_fn(msi_handler_51);
        idt[52].set_handler_fn(msi_handler_52);
        idt[53].set_handler_fn(msi_handler_53);
        idt[54].set_handler_fn(msi_handler_54);
        idt[55].set_handler_fn(msi_handler_55);
        idt[56].set_handler_fn(msi_handler_56);
        idt[57].set_handler_fn(msi_handler_57);
        idt[58].set_handler_fn(msi_handler_58);
        idt[59].set_handler_fn(msi_handler_59);
        idt[60].set_handler_fn(msi_handler_60);
        idt[61].set_handler_fn(msi_handler_61);
        idt[62].set_handler_fn(msi_handler_62);
        idt[63].set_handler_fn(msi_handler_63);

        idt
    });
    idt.load();
}

fn dispatch_dynamic_irq(vector: u8) {
    record_irq(vector);
    if let Some(idx) = vector.checked_sub(PIC_1_OFFSET) {
        if let Some(table) = IRQ_HANDLERS.try_lock() {
            if let Some(handler) = table.get(idx as usize).and_then(|e| e.handler) {
                drop(table);
                handler(vector);
            }
        }
    }
    end_of_interrupt(vector);
}

macro_rules! def_msi_handler {
    ($name:ident, $vec:expr) => {
        extern "x86-interrupt" fn $name(_stack_frame: InterruptStackFrame) {
            dispatch_dynamic_irq($vec);
        }
    };
}

def_msi_handler!(msi_handler_48, 48);
def_msi_handler!(msi_handler_49, 49);
def_msi_handler!(msi_handler_50, 50);
def_msi_handler!(msi_handler_51, 51);
def_msi_handler!(msi_handler_52, 52);
def_msi_handler!(msi_handler_53, 53);
def_msi_handler!(msi_handler_54, 54);
def_msi_handler!(msi_handler_55, 55);
def_msi_handler!(msi_handler_56, 56);
def_msi_handler!(msi_handler_57, 57);
def_msi_handler!(msi_handler_58, 58);
def_msi_handler!(msi_handler_59, 59);
def_msi_handler!(msi_handler_60, 60);
def_msi_handler!(msi_handler_61, 61);
def_msi_handler!(msi_handler_62, 62);
def_msi_handler!(msi_handler_63, 63);

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
    let is_user_addr = accessed_addr
        .as_ref()
        .map(|a| a.as_u64() < 0x0000_8000_0000_0000)
        .unwrap_or(false);

    if is_user || is_user_addr {
        if let Ok(addr) = accessed_addr {
            if crate::memory::handle_user_page_fault(addr, error_code).is_ok() {
                return;
            }
        }
        if is_user {
            let rsp = stack_frame.stack_pointer.as_u64();
            serial_println!(
                "[kernel] #PF in Ring 3 tgid={} exe={} at {:?}, error: {:?}, RIP {:#x}, RSP {:#x} - terminating user task",
                crate::task::current_thread_tgid(),
                crate::task::current_thread_exe_path(),
                accessed_addr,
                error_code,
                stack_frame.instruction_pointer.as_u64(),
                rsp
            );
            for i in 0..8u64 {
                let addr = rsp + i * 8;
                if crate::task::syscall::validate_user_buffer(addr, 8) {
                    let val = unsafe { core::ptr::read_volatile(addr as *const u64) };
                    serial_println!("  [rsp+{:#x}] = {:#x}", i * 8, val);
                }
            }
            crate::task::syscall::sys_exit(128 + 14);
        } else if is_user_addr {
            serial_println!(
                "[kernel] #PF in Ring 0 accessing user address {:?}, error: {:?}, RIP {:#x} - terminating user task tgid={}",
                accessed_addr,
                error_code,
                stack_frame.instruction_pointer.as_u64(),
                crate::task::current_thread_tgid()
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
        serial_println!(
            "EXCEPTION: GENERAL PROTECTION FAULT (code: {:#x})",
            error_code
        );
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
    record_irq(InterruptIndex::Timer.as_u8());
    super::timer::on_tick();
    crate::net::poll();

    let mut rx_count = 0;
    while !crate::fs::devfs::tty::is_serial_probe_active() {
        let Some(b) = crate::drivers::serial::try_receive() else {
            break;
        };
        crate::fs::devfs::tty::push_serial_rx_byte(b);
        rx_count += 1;
        if rx_count >= 64 {
            break;
        }
    }
    if !crate::fs::devfs::tty::is_serial_probe_active() {
        crate::fs::devfs::tty::flush_stale_serial_esc();
    }

    // Отправляем EOI ДО переключения контекста, чтобы следующий тик не был заблокирован
    end_of_interrupt(InterruptIndex::Timer.as_u8());

    let is_user = (stack_frame.code_segment.0 & 3) == 3;
    if is_user
        && crate::task::is_current_thread_sync_child()
        && crate::task::signal::is_sigint_pending_for_current()
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

    record_irq(InterruptIndex::Keyboard.as_u8());
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    crate::drivers::keyboard::handle_scancode(scancode);
    end_of_interrupt(InterruptIndex::Keyboard.as_u8());
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    record_irq(InterruptIndex::Mouse.as_u8());
    crate::drivers::input::ps2_mouse::handle_interrupt();
    end_of_interrupt(InterruptIndex::Mouse.as_u8());
}
