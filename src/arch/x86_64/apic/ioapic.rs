use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::hhdm_offset;

static IOAPIC_BASE_PHYS: AtomicU64 = AtomicU64::new(0xFEC0_0000);

const _IOREGSEL: usize = 0x00;
const IOWIN: usize = 0x10;

unsafe fn read_reg(reg: u8) -> u32 {
    let base = hhdm_offset() + IOAPIC_BASE_PHYS.load(Ordering::Relaxed);
    unsafe {
        core::ptr::write_volatile(base as *mut u32, reg as u32);
        core::ptr::read_volatile((base + IOWIN as u64) as *const u32)
    }
}

unsafe fn write_reg(reg: u8, val: u32) {
    let base = hhdm_offset() + IOAPIC_BASE_PHYS.load(Ordering::Relaxed);
    unsafe {
        core::ptr::write_volatile(base as *mut u32, reg as u32);
        core::ptr::write_volatile((base + IOWIN as u64) as *mut u32, val);
    }
}

/// Инициализирует I/O APIC контроллер по физическому адресу
pub fn init(phys_addr: u64) {
    if phys_addr != 0 {
        IOAPIC_BASE_PHYS.store(phys_addr, Ordering::Relaxed);
    }

    let id = unsafe { (read_reg(0x00) >> 24) & 0x0F };
    let ver_reg = unsafe { read_reg(0x01) };
    let max_redir = ((ver_reg >> 16) & 0xFF) + 1;

    crate::klog!("ioapic: initialized (ID: {}, lines: {})", id, max_redir);

    // По умолчанию маскируем все входные линии (Redirection Tables)
    for pin in 0..max_redir as u8 {
        set_redirection(pin, 0, 0, true, false, false);
    }
}

/// Настраивает перенаправление прерывания с физической ножки I/O APIC на вектор CPU
pub fn set_redirection(
    pin: u8,
    vector: u8,
    dest_apic_id: u8,
    masked: bool,
    level_triggered: bool,
    active_low: bool,
) {
    let reg_low = 0x10 + pin * 2;
    let reg_high = reg_low + 1;

    let mut low = vector as u32;
    if masked {
        low |= 1 << 16;
    }
    if level_triggered {
        low |= 1 << 15;
    }
    if active_low {
        low |= 1 << 13;
    }

    let high = (dest_apic_id as u32) << 24;

    unsafe {
        write_reg(reg_low, low);
        write_reg(reg_high, high);
    }
}
