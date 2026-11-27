use alloc::vec::Vec;
use super::DriverInfo;
use crate::i18n::t;

/// Добавляет только реально присутствующие и активные системные драйверы платформы x86_64.
pub fn append_system_drivers(list: &mut Vec<DriverInfo>) {
    list.push(DriverInfo {
        name: "ps2_keyboard",
        description: t(
            "Драйвер PS/2 клавиатуры (Scancode Set 1, EN/RU)",
            "Standard PS/2 Keyboard Driver (Scancode Set 1)",
        ),
        category: "input",
        version: env!("CARGO_PKG_VERSION"),
        author: "Wunix Kernel Team",
        license: "MIT",
        devices_count: 1,
        status: "Live",
        size: core::mem::size_of::<crate::drivers::keyboard::RingBuffer<u8, 256>>() + 8192,
        dependencies: "-",
    });

    if crate::drivers::input::ps2_mouse::is_present() {
        list.push(DriverInfo {
            name: "ps2_mouse",
            description: t(
                "Драйвер PS/2 мыши (IRQ12, IntelliMouse)",
                "PS/2 Mouse & IntelliMouse Driver (IRQ12)",
            ),
            category: "input",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: core::mem::size_of::<crate::drivers::input::ps2_mouse::Ps2Mouse>() + 4096,
            dependencies: "-",
        });
    }

    if crate::drivers::serial::is_present() {
        list.push(DriverInfo {
            name: "uart16550",
            description: t(
                "Драйвер последовательного порта UART 16550 (COM1 0x3F8)",
                "UART 16550 Serial COM Port Driver (COM1 0x3F8)",
            ),
            category: "char",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 8192,
            dependencies: "-",
        });
    }

    if crate::drivers::display::is_present() {
        list.push(DriverInfo {
            name: "limine_fb",
            description: t(
                "Драйвер линейного кадрового буфера (VT100 консоль)",
                "Linear Framebuffer Display Driver (VT100 Console)",
            ),
            category: "display",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: core::mem::size_of::<crate::drivers::display::DisplayConsole>() + 8192,
            dependencies: "-",
        });
    }

    list.push(DriverInfo {
        name: "rtc_cmos",
        description: t(
            "Часы реального времени CMOS MC146818",
            "Motorola MC146818 CMOS Real-Time Clock",
        ),
        category: "system",
        version: env!("CARGO_PKG_VERSION"),
        author: "Wunix Kernel Team",
        license: "MIT",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "pit8254",
        description: t(
            "Программируемый интервальный таймер Intel 8254 (100 Гц)",
            "Intel 8254 Programmable Interval Timer (100 Hz Scheduler)",
        ),
        category: "system",
        version: env!("CARGO_PKG_VERSION"),
        author: "Wunix Kernel Team",
        license: "MIT",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    if crate::arch::x86_64::apic::is_active() {
        list.push(DriverInfo {
            name: "ioapic",
            description: t(
                "Контроллеры прерываний Local APIC и I/O APIC (MSI/GSI)",
                "Local APIC and I/O APIC Interrupt Controllers (MSI/GSI)",
            ),
            category: "system",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 8192,
            dependencies: "acpi_core",
        });
    }

    list.push(DriverInfo {
        name: "i8259_pic",
        description: t(
            "Сдвоенный контроллер прерываний 8259 PIC (векторы 32..47)",
            "Dual 8259 Programmable Interrupt Controller (Remapped 32..47)",
        ),
        category: "system",
        version: env!("CARGO_PKG_VERSION"),
        author: "Wunix Kernel Team",
        license: "MIT",
        devices_count: 1,
        status: if crate::arch::x86_64::apic::is_active() {
            "Masked"
        } else {
            "Live"
        },
        size: 4096,
        dependencies: "-",
    });

    if crate::drivers::acpi::is_present() {
        list.push(DriverInfo {
            name: "acpi_core",
            description: t(
                "Подсистема таблиц и питания ACPI 2.0+ (FADT, MADT, DSDT S5)",
                "ACPI 2.0+ Tables & Power Subsystem (FADT, MADT, DSDT S5)",
            ),
            category: "system",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 16384,
            dependencies: "-",
        });
    }

    if crate::drivers::tpm::is_present() {
        list.push(DriverInfo {
            name: "tpm2_crb",
            description: t(
                "Модуль безопасности TPM 2.0 CRB (TRNG)",
                "TPM 2.0 Command Response Buffer (TRNG)",
            ),
            category: "security",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 8192,
            dependencies: "-",
        });
    }

    if crate::arch::x86_64::cpu::has_rdrand() {
        list.push(DriverInfo {
            name: "rdrand_rng",
            description: t(
                "Аппаратный генератор случайных чисел CPU (RDRAND)",
                "CPU Hardware Random Number Generator (RDRAND)",
            ),
            category: "security",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 4096,
            dependencies: "-",
        });
    }
}
