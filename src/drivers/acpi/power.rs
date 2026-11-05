use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use x86_64::instructions::port::Port;
use super::tables::Fadt;

static PM1A_CNT_BLK: AtomicU32 = AtomicU32::new(0);
static PM1B_CNT_BLK: AtomicU32 = AtomicU32::new(0);
static SLP_TYPA: AtomicU8 = AtomicU8::new(5); // S5 sleep state (Soft Off)
static SLP_TYPB: AtomicU8 = AtomicU8::new(5);
static RESET_REG_PORT: AtomicU32 = AtomicU32::new(0);
static RESET_REG_VAL: AtomicU8 = AtomicU8::new(0);
static ACPI_POWER_READY: AtomicBool = AtomicBool::new(false);

/// Инициализирует параметры управления питанием из разобранной таблицы FADT
pub fn init_power_from_fadt(fadt: &Fadt) {
    let pm1a = fadt.pm1a_control_block;
    let pm1b = fadt.pm1b_control_block;
    if pm1a != 0 {
        PM1A_CNT_BLK.store(pm1a, Ordering::SeqCst);
    }
    if pm1b != 0 {
        PM1B_CNT_BLK.store(pm1b, Ordering::SeqCst);
    }

    if fadt.reset_reg.address_space == 1 && fadt.reset_reg.address != 0 {
        RESET_REG_PORT.store(fadt.reset_reg.address as u32, Ordering::SeqCst);
        RESET_REG_VAL.store(fadt.reset_value, Ordering::SeqCst);
    }

    ACPI_POWER_READY.store(true, Ordering::SeqCst);
}

/// Выполняет корректное аппаратное выключение системы (ACPI S5 Soft Off)
pub fn power_off() -> ! {
    let pm1a = PM1A_CNT_BLK.load(Ordering::SeqCst);
    let slp_a = SLP_TYPA.load(Ordering::SeqCst);

    if pm1a != 0 {
        // Бит 13 в регистре PM1_CNT — SLP_EN (Sleep Enable), биты 10..12 — SLP_TYP
        let val: u16 = ((slp_a as u16 & 0x07) << 10) | (1 << 13);
        let mut port = Port::<u16>::new(pm1a as u16);
        unsafe {
            port.write(val);
        }

        let pm1b = PM1B_CNT_BLK.load(Ordering::SeqCst);
        if pm1b != 0 {
            let slp_b = SLP_TYPB.load(Ordering::SeqCst);
            let val_b: u16 = ((slp_b as u16 & 0x07) << 10) | (1 << 13);
            let mut port_b = Port::<u16>::new(pm1b as u16);
            unsafe {
                port_b.write(val_b);
            }
        }
    }

    // Запасные выходы для распространенных гипервизоров и чипсетов
    unsafe {
        // QEMU modern / microvm ACPI shutdown port
        let mut qemu_port = Port::<u16>::new(0x604);
        qemu_port.write(0x2000);

        // Bochs / старый QEMU
        let mut bochs_port = Port::<u16>::new(0xB004);
        bochs_port.write(0x2000);

        // VirtualBox
        let mut vbox_port = Port::<u16>::new(0x4004);
        vbox_port.write(0x3400);

        // Cloud Hypervisor
        let mut cloud_port = Port::<u8>::new(0x600);
        cloud_port.write(0x34);
    }

    // Если питание не отключилось аппаратно, глушим CPU
    loop {
        x86_64::instructions::hlt();
    }
}

/// Выполняет аппаратный перезапуск системы
pub fn reboot() -> ! {
    let reset_port = RESET_REG_PORT.load(Ordering::SeqCst);
    let reset_val = RESET_REG_VAL.load(Ordering::SeqCst);

    // 1. Метод ACPI FADT Reset Register
    if reset_port != 0 {
        let mut p = Port::<u8>::new(reset_port as u16);
        unsafe {
            p.write(reset_val);
        }
    }

    // 2. Метод сброса через контроллер клавиатуры i8042 (импульс на ножку CPU RESET)
    unsafe {
        let mut cmd_port = Port::<u8>::new(0x64);
        // Ждем готовности буфера i8042
        for _ in 0..10_000 {
            if (cmd_port.read() & 0x02) == 0 {
                break;
            }
        }
        cmd_port.write(0xFE);
    }

    // 3. Крайний метод — намеренный Triple Fault через сброс IDT в ноль
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

/// Сведения о питании системы для отображения в /sys/class/power_supply
pub struct PowerSupplyInfo {
    pub ac_online: bool,
    pub battery_present: bool,
    pub battery_percent: u8,
    pub battery_status: &'static str,
}

pub fn get_power_supply_info() -> PowerSupplyInfo {
    // В реальной системе статус читается через ACPI Embedded Controller (EC) или SMBus
    PowerSupplyInfo {
        ac_online: true,
        battery_present: true,
        battery_percent: 100,
        battery_status: "Full",
    }
}
