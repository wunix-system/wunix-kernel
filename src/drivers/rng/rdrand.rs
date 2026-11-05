/// Проверяет поддержку процессорной инструкции RDRAND (CPUID leaf 1, ECX бит 30)
pub fn has_rdrand() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    (ecx & (1 << 30)) != 0
}

/// Считывает аппаратное случайное 64-битное число через инструкцию RDRAND
#[inline]
pub fn rdrand64() -> Option<u64> {
    let mut val: u64;
    let success: u8;
    unsafe {
        core::arch::asm!(
            "rdrand {0}",
            "setc {1}",
            out(reg) val,
            out(reg_byte) success,
        );
    }
    if success == 1 {
        Some(val)
    } else {
        None
    }
}

/// Заполняет буфер криптографической энтропией из аппаратного генератора CPU
pub fn fill_random(buf: &mut [u8]) -> bool {
    if !has_rdrand() {
        return false;
    }

    let mut offset = 0;
    while offset < buf.len() {
        // До 10 попыток считывания энтропии на случай разрядки пула конденсаторов TRNG
        let mut num = None;
        for _ in 0..10 {
            if let Some(r) = rdrand64() {
                num = Some(r);
                break;
            }
        }
        let rand_val = match num {
            Some(v) => v,
            None => return false,
        };

        let bytes = rand_val.to_le_bytes();
        let chunk = (buf.len() - offset).min(8);
        buf[offset..offset + chunk].copy_from_slice(&bytes[..chunk]);
        offset += chunk;
    }

    true
}
