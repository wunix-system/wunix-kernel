use core::arch::x86_64::__cpuid;

/// Данные идентификации процессора, считанные через инструкцию CPUID.
#[derive(Debug, Clone)]
pub struct CpuInfo {
    vendor_bytes: [u8; 12],
    brand_bytes: [u8; 48],
}

impl CpuInfo {
    /// Считывает строку производителя и коммерческое наименование процессора (Brand String).
    pub fn current() -> Self {
        let mut vendor_bytes = [0u8; 12];
        let mut brand_bytes = [0u8; 48];

        // Leaf 0: Vendor String (EBX, EDX, ECX)
        let leaf0 = __cpuid(0);
        vendor_bytes[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
        vendor_bytes[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
        vendor_bytes[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

        // Leaf 0x8000_0000: максимальный номер расширенной функции CPUID
        let ext_max = __cpuid(0x8000_0000).eax;
        if ext_max >= 0x8000_0004 {
            let mut offset = 0;
            for leaf in 0x8000_0002..=0x8000_0004 {
                let res = __cpuid(leaf);
                brand_bytes[offset..offset + 4].copy_from_slice(&res.eax.to_le_bytes());
                brand_bytes[offset + 4..offset + 8].copy_from_slice(&res.ebx.to_le_bytes());
                brand_bytes[offset + 8..offset + 12].copy_from_slice(&res.ecx.to_le_bytes());
                brand_bytes[offset + 12..offset + 16].copy_from_slice(&res.edx.to_le_bytes());
                offset += 16;
            }
        }

        Self {
            vendor_bytes,
            brand_bytes,
        }
    }

    /// Возвращает имя производителя CPU (например, "GenuineIntel", "AuthenticAMD").
    pub fn vendor(&self) -> &str {
        core::str::from_utf8(&self.vendor_bytes).unwrap_or("Unknown")
    }

    /// Возвращает коммерческое наименование процессора (например, "QEMU Virtual CPU version 2.5+").
    pub fn brand(&self) -> &str {
        let raw = core::str::from_utf8(&self.brand_bytes).unwrap_or("");
        let trimmed = raw.trim_matches(|c: char| c == ' ' || c == '\0');
        if trimmed.is_empty() {
            "Generic x86_64 Processor"
        } else {
            trimmed
        }
    }
}

/// Инициализирует математический сопроцессор x87 FPU и блок расширений SSE/SIMD:
/// - Сбрасывает флаг эмуляции CR0.EM и выставляет CR0.MP и CR0.NE
/// - Включает CR4.OSFXSR (поддержка FXSAVE/FXRSTOR и 128-битных регистров XMM)
/// - Включает CR4.OSXMMEXCPT_ENABLE (обработка SIMD-исключений)
/// - Выполняет инструкцию fninit для сброса контрольных слов FPU
pub fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

    unsafe {
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::NUMERIC_ERROR);
        Cr0::write(cr0);

        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);

        core::arch::asm!("fninit");
    }
}

/// Буфер аппаратного состояния x87 FPU и SSE/XMM (формат инструкции `fxsave64`).
/// Выравнивание на 16 байт требуется самой инструкцией x86_64 во избежание `#GP`.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FpuState(pub [u8; 512]);

impl FpuState {
    pub fn initial() -> Self {
        let mut s = Self([0u8; 512]);
        // Маскируем все исключения x87 (FCW = 0x037F) и SSE (MXCSR = 0x1F80) по ABI System V.
        s.0[0] = 0x7f;
        s.0[1] = 0x03;
        s.0[24] = 0x80;
        s.0[25] = 0x1f;
        s
    }

    pub fn save_current() -> Self {
        let mut s = Self([0u8; 512]);
        s.save();
        s
    }

    #[inline]
    pub fn save(&mut self) {
        unsafe {
            core::arch::asm!(
                "fxsave64 [{}]",
                in(reg) self.0.as_mut_ptr(),
                options(nostack)
            );
        }
    }

    #[inline]
    pub fn restore(&self) {
        unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) self.0.as_ptr(),
                options(nostack)
            );
        }
    }
}

/// Выключает питание виртуальной машины или компьютера.
/// Использует порты управления питанием ACPI (QEMU 0x604, Bochs 0xb004, VBox 0x4004)
/// и порт отладочного выхода QEMU 0xf4.
pub fn poweroff() -> ! {
    use x86_64::instructions::port::Port;

    x86_64::instructions::interrupts::disable();

    unsafe {
        // QEMU modern ACPI shutdown
        let mut p604 = Port::<u16>::new(0x604);
        p604.write(0x2000);
        p604.write(0x3400);

        // Bochs / older QEMU ACPI shutdown
        let mut pb004 = Port::<u16>::new(0xb004);
        pb004.write(0x2000);

        // VirtualBox poweroff
        let mut p4004 = Port::<u16>::new(0x4004);
        p4004.write(0x3400);

        // QEMU isa-debug-exit device (код 0x10 = успех)
        let mut pf4 = Port::<u32>::new(0xf4);
        pf4.write(0x10);

        // APM BIOS shutdown
        let mut p5301 = Port::<u16>::new(0x5301);
        p5301.write(0x5307);
    }

    loop {
        x86_64::instructions::hlt();
    }
}

/// Выполняет аппаратный перезапуск компьютера.
/// Последовательно пытается выполнить:
/// 1. Быстрый сброс через Fast A20/Reset порт 0x92
/// 2. Импульс сброса через контроллер клавиатуры 8042 (порт 0x64, команда 0xFE)
/// 3. Сброс через чипсет PCI (порт 0xCF9, значение 0x06)
/// 4. Гарантированный Triple Fault процессора через сброс IDT
pub fn reboot() -> ! {
    use x86_64::instructions::port::Port;

    x86_64::instructions::interrupts::disable();

    unsafe {
        // 1. Fast reset (System Control Port A)
        let mut p92 = Port::<u8>::new(0x92);
        let val = p92.read();
        p92.write(val | 0x01);

        // 2. 8042 PS/2 Keyboard Controller pulse reset
        let mut p64 = Port::<u8>::new(0x64);
        for _ in 0..10_000 {
            if (p64.read() & 0x02) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        p64.write(0xfe);

        // 3. PCI reset control register (ICH/PCH)
        let mut pcf9 = Port::<u8>::new(0xcf9);
        pcf9.write(0x06);

        // 4. Fallback: Triple Fault (загружаем нулевой лимит IDT и вызываем исключение)
        let null_idt_ptr: [u16; 5] = [0, 0, 0, 0, 0];
        core::arch::asm!(
            "lidt [{}]",
            "ud2",
            in(reg) null_idt_ptr.as_ptr(),
            options(noreturn)
        );
    }
}

/// Переводит процессор в состояние полной остановки с запретом прерываний (HALT).
pub fn halt() -> ! {
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

static HAS_RDRAND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn has_rdrand() -> bool {
    match HAS_RDRAND.load(core::sync::atomic::Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let supported = (core::arch::x86_64::__cpuid(1).ecx & (1 << 30)) != 0;
            HAS_RDRAND.store(if supported { 1 } else { 2 }, core::sync::atomic::Ordering::Relaxed);
            supported
        }
    }
}

/// Заполняет переданный буфер энтропией из аппаратного генератора RDRAND или RDTSC PRNG
pub fn get_random_bytes(buf: &mut [u8]) {
    let mut seed = unsafe { core::arch::x86_64::_rdtsc() };
    let mut i = 0;
    while i < buf.len() {
        let mut val: u64 = 0;
        let mut ok = false;
        if has_rdrand() {
            let mut r_val: u64 = 0;
            let mut r_ok: u8 = 0;
            unsafe {
                core::arch::asm!(
                    "rdrand {0}",
                    "setc {1}",
                    out(reg) r_val,
                    out(reg_byte) r_ok,
                );
            }
            if r_ok != 0 {
                val = r_val;
                ok = true;
            }
        }
        if !ok {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            val = seed;
        }
        let bytes = val.to_ne_bytes();
        let to_copy = (buf.len() - i).min(8);
        buf[i..i + to_copy].copy_from_slice(&bytes[..to_copy]);
        i += to_copy;
    }
}

