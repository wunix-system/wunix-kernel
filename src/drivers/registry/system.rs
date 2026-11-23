use alloc::vec::Vec;
use super::DriverInfo;

/// Добавляет базовые драйверы процессорной платформы и системного чипсета x86_64
pub fn append_system_drivers(list: &mut Vec<DriverInfo>) {
    list.push(DriverInfo {
        name: "ps2_keyboard",
        description: "Standard PS/2 Keyboard Driver (Scancode Set 1)",
        category: "input",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 12288,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "ps2_mouse",
        description: "PS/2 Mouse & IntelliMouse Driver (IRQ12)",
        category: "input",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 10240,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "uart16550",
        description: "UART 16550 Serial COM Port Driver (COM1 0x3F8)",
        category: "char",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 8192,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "limine_fb",
        description: "Limine Linear Framebuffer Display Driver (VT100 Console)",
        category: "display",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 16384,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "rtc_cmos",
        description: "Motorola MC146818 CMOS Real-Time Clock",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "pit8254",
        description: "Intel 8254 Programmable Interval Timer (1000 Hz Scheduler)",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "i8259_pic",
        description: "Dual 8259 Programmable Interrupt Controller (Remapped 32..47)",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "acpi_core",
        description: "ACPI 2.0+ Tables & Power Subsystem (FADT, MADT, S5 Off, Reset)",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 16384,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "tpm2_crb",
        description: "TPM 2.0 Command Response Buffer (Intel PTT / AMD fTPM TRNG)",
        category: "security",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 8192,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "rdrand_rng",
        description: "Intel/AMD Hardware Random Number Generator (RDRAND/RDSEED)",
        category: "security",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });
}
