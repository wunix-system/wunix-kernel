use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::hhdm_offset;

// Базовый адрес Local APIC в физической памяти (по умолчанию 0xFEE00000)
static LAPIC_BASE_PHYS: AtomicU64 = AtomicU64::new(0xFEE0_0000);
static LAPIC_MMIO_VIRT: AtomicU64 = AtomicU64::new(0);

// Смещения регистров Local APIC
const LAPIC_ID: usize = 0x020;
const LAPIC_VERSION: usize = 0x030;
const LAPIC_TPR: usize = 0x080;
const LAPIC_EOI: usize = 0x0B0;
const LAPIC_SVR: usize = 0x0F0;
const _LAPIC_TIMER_INIT: usize = 0x380;
const LAPIC_TIMER_DIV: usize = 0x3E0;
const _LAPIC_LVT_TIMER: usize = 0x320;
const LAPIC_LVT_ERROR: usize = 0x370;

unsafe fn read_reg(offset: usize) -> u32 {
    let mut base = LAPIC_MMIO_VIRT.load(Ordering::Relaxed);
    if base == 0 {
        base = hhdm_offset() + LAPIC_BASE_PHYS.load(Ordering::Relaxed);
    }
    unsafe { core::ptr::read_volatile((base + offset as u64) as *const u32) }
}

unsafe fn write_reg(offset: usize, value: u32) {
    let mut base = LAPIC_MMIO_VIRT.load(Ordering::Relaxed);
    if base == 0 {
        base = hhdm_offset() + LAPIC_BASE_PHYS.load(Ordering::Relaxed);
    }
    unsafe { core::ptr::write_volatile((base + offset as u64) as *mut u32, value); }
}

/// Инициализирует Local APIC текущего процессорного ядра
pub fn init(phys_addr: u64) {
    let paddr = if phys_addr != 0 { phys_addr } else { 0xFEE0_0000 };
    LAPIC_BASE_PHYS.store(paddr, Ordering::Relaxed);

    let mmio_virt = match crate::memory::map_kernel_mmio(paddr, 4096) {
        Ok(v) => v.as_u64(),
        Err(_) => hhdm_offset() + paddr,
    };
    LAPIC_MMIO_VIRT.store(mmio_virt, Ordering::Relaxed);

    unsafe {
        // Проверяем бит APIC Global Enable в регистре MSR 0x1B (IA32_APIC_BASE)
        let mut apic_base_msr = x86_64::registers::model_specific::Msr::new(0x1B);
        let val = apic_base_msr.read();
        if (val & (1 << 11)) == 0 {
            // Включаем бит 11 (APIC Global Enable)
            apic_base_msr.write(val | (1 << 11));
        }

        // Обнуляем Task Priority Register (разрешаем обработку всех классов приоритетов)
        write_reg(LAPIC_TPR, 0);

        // Включаем Spurious Interrupt Vector Register (бит 8 = APIC Software Enable, вектор = 0xFF)
        write_reg(LAPIC_SVR, 0x100 | 0xFF);

        // Маскируем прерывание ошибки LVT Error
        write_reg(LAPIC_LVT_ERROR, 0x10000);

        // Настраиваем таймер APIC: делитель частоты на 16
        write_reg(LAPIC_TIMER_DIV, 0x03);

        // Отправляем первый EOI для очистки зависших транзакций
        write_reg(LAPIC_EOI, 0);
    }

    let apic_id = id();
    let version = unsafe { read_reg(LAPIC_VERSION) & 0xFF };
    crate::klog!("lapic: initialized (ID: {}, version: {:#x})", apic_id, version);
}

/// Возвращает аппаратный идентификатор Local APIC
pub fn id() -> u8 {
    unsafe { ((read_reg(LAPIC_ID) >> 24) & 0xFF) as u8 }
}

/// Отправляет подтверждение окончания обработки прерывания (End of Interrupt)
#[inline]
pub fn eoi() {
    unsafe {
        write_reg(LAPIC_EOI, 0);
    }
}
