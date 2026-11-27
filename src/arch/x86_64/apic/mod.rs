pub mod ioapic;
pub mod lapic;

use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::instructions::port::Port;
use crate::drivers::acpi;

static APIC_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn is_active() -> bool {
    APIC_ACTIVE.load(Ordering::Acquire)
}

/// Отключает устаревший контроллер прерываний Intel 8259 PIC, маскируя все линии.
pub fn disable_pic() {
    unsafe {
        let mut pic1_data = Port::<u8>::new(0x21);
        let mut pic2_data = Port::<u8>::new(0xA1);
        pic1_data.write(0xFF);
        pic2_data.write(0xFF);
    }
}

/// Разрешает конкретную ISA-линию IRQ в I/O APIC с учетом таблиц Interrupt Source Override (MADT).
pub fn route_isa_irq(isa_irq: u8, vector: u8) {
    let madt = match acpi::get_madt_info() {
        Some(m) => m,
        None => return,
    };
    let mut gsi = isa_irq as u32;
    let mut active_low = false;
    let mut level_triggered = false;

    for iso in &madt.interrupt_source_overrides {
        if iso.source == isa_irq {
            gsi = iso.global_system_interrupt;
            let pol = iso.flags & 0x03;
            let trig = (iso.flags >> 2) & 0x03;
            active_low = pol == 3;
            level_triggered = trig == 3;
            break;
        }
    }

    let dest = lapic::id();
    ioapic::set_redirection(gsi as u8, vector, dest, false, level_triggered, active_low);
}

/// Инициализирует Local APIC и I/O APIC на основе данных ACPI MADT и переводит ISA IRQ на IOAPIC.
pub fn init() {
    let madt = match acpi::get_madt_info() {
        Some(m) => m,
        None => {
            crate::serial_println!("apic: MADT not available, staying in PIC mode");
            return;
        }
    };

    if madt.io_apics.is_empty() {
        return;
    }

    x86_64::instructions::interrupts::without_interrupts(|| {
        disable_pic();
        lapic::init(madt.local_apic_phys);
        if let Some(io_apic) = madt.io_apics.first() {
            ioapic::init(io_apic.io_apic_address as u64);
        }
        // Перенаправляем PIT (IRQ0 -> vec 32) и клавиатуру (IRQ1 -> vec 33)
        route_isa_irq(0, super::interrupts::PIC_1_OFFSET);
        route_isa_irq(1, super::interrupts::PIC_1_OFFSET + 1);
        lapic::eoi();
        APIC_ACTIVE.store(true, Ordering::Release);
    });

    crate::kboot!(
        "apic0",
        "{}",
        crate::i18n::t(
            "контроллеры Local APIC и I/O APIC активны",
            "Local APIC and I/O APIC active"
        )
    );
}
