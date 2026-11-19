pub mod ioapic;
pub mod lapic;

use x86_64::instructions::port::Port;
use crate::drivers::acpi;

/// Отключает устаревший контроллер прерываний Intel 8259 PIC
pub fn disable_pic() {
    unsafe {
        let mut pic1_data = Port::<u8>::new(0x21);
        let mut pic2_data = Port::<u8>::new(0xA1);
        pic1_data.write(0xFF);
        pic2_data.write(0xFF);
    }
}

/// Инициализирует Local APIC и I/O APIC на основе данных ACPI MADT
pub fn init() {
    let madt = match acpi::get_madt_info() {
        Some(m) => m,
        None => {
            crate::klog!("apic: MADT not available, staying in PIC mode");
            return;
        }
    };

    lapic::init(madt.local_apic_phys);

    if let Some(io_apic) = madt.io_apics.first() {
        ioapic::init(io_apic.io_apic_address as u64);
    }
}
