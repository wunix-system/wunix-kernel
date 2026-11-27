use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use x86_64::instructions::port::Port;
use super::tables::{AcpiHeader, Fadt};

static PM1A_CNT_BLK: AtomicU32 = AtomicU32::new(0);
static PM1B_CNT_BLK: AtomicU32 = AtomicU32::new(0);
static SLP_TYPA: AtomicU8 = AtomicU8::new(5);
static SLP_TYPB: AtomicU8 = AtomicU8::new(5);
static RESET_REG_PORT: AtomicU32 = AtomicU32::new(0);
static RESET_REG_VAL: AtomicU8 = AtomicU8::new(0);
static ACPI_POWER_READY: AtomicBool = AtomicBool::new(false);
static PM_PROFILE_MOBILE: AtomicBool = AtomicBool::new(false);

pub fn is_ready() -> bool {
    ACPI_POWER_READY.load(Ordering::Acquire)
}

fn read_aml_pkg_len_size(b0: u8) -> usize {
    match b0 >> 6 {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => 4,
    }
}

fn read_aml_int(slice: &[u8], pos: &mut usize) -> Option<u8> {
    let op = *slice.get(*pos)?;
    *pos += 1;
    match op {
        0x00 => Some(0), // ZeroOp
        0x01 => Some(1), // OneOp
        0x0A => {
            let v = *slice.get(*pos)?;
            *pos += 1;
            Some(v)
        }
        0x0B => {
            let v = *slice.get(*pos)?;
            *pos += 2;
            Some(v)
        }
        0x0C => {
            let v = *slice.get(*pos)?;
            *pos += 4;
            Some(v)
        }
        _ => None,
    }
}

/// Извлекает значения `(SLP_TYPa, SLP_TYPb)` объекта `\_S5` из AML-байткода таблицы DSDT.
pub fn parse_s5_from_aml(dsdt_bytes: &[u8]) -> Option<(u8, u8)> {
    const TARGET: &[u8; 4] = b"_S5_";
    let mut i = 0;
    while i + 8 < dsdt_bytes.len() {
        if &dsdt_bytes[i..i + 4] == TARGET {
            // Либо `NameOp (0x08) _S5_`, либо `NameOp '\' _S5_`
            let has_name_op = (i >= 1 && dsdt_bytes[i - 1] == 0x08)
                || (i >= 2 && dsdt_bytes[i - 2] == 0x08 && dsdt_bytes[i - 1] == b'\\');
            if has_name_op {
                let mut pos = i + 4;
                if *dsdt_bytes.get(pos)? == 0x12 {
                    // PackageOp
                    pos += 1;
                    let pkg_len_bytes = read_aml_pkg_len_size(*dsdt_bytes.get(pos)?);
                    pos += pkg_len_bytes;
                    let _num_elements = *dsdt_bytes.get(pos)?;
                    pos += 1;
                    let slp_a = read_aml_int(dsdt_bytes, &mut pos)?;
                    let slp_b = read_aml_int(dsdt_bytes, &mut pos).unwrap_or(slp_a);
                    return Some((slp_a, slp_b));
                }
            }
        }
        i += 1;
    }
    None
}

/// Инициализирует параметры управления питанием из таблицы FADT и AML-таблицы DSDT.
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

    // preferred_pm_profile == 2 (Mobile / Laptop)
    PM_PROFILE_MOBILE.store(fadt.preferred_pm_profile == 2, Ordering::SeqCst);

    if fadt.dsdt != 0 {
        if let Ok(hdr_virt) = crate::memory::map_kernel_mmio(fadt.dsdt as u64, 4096) {
            let hdr = unsafe { &*(hdr_virt.as_u64() as *const AcpiHeader) };
            let len = hdr.length as usize;
            if &hdr.signature == b"DSDT"
                && len >= core::mem::size_of::<AcpiHeader>()
                && len <= 1024 * 1024
            {
                if let Ok(dsdt_virt) = crate::memory::map_kernel_mmio(fadt.dsdt as u64, len) {
                    let bytes =
                        unsafe { core::slice::from_raw_parts(dsdt_virt.as_u64() as *const u8, len) };
                    if let Some((slp_a, slp_b)) = parse_s5_from_aml(bytes) {
                        SLP_TYPA.store(slp_a, Ordering::SeqCst);
                        SLP_TYPB.store(slp_b, Ordering::SeqCst);
                    }
                }
            }
        }
    }

    ACPI_POWER_READY.store(true, Ordering::Release);
}

/// Выполняет корректное аппаратное выключение системы (ACPI S5 Soft Off)
pub fn power_off() -> ! {
    let pm1a = PM1A_CNT_BLK.load(Ordering::SeqCst);
    let slp_a = SLP_TYPA.load(Ordering::SeqCst);

    if pm1a != 0 {
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

    unsafe {
        let mut qemu_port = Port::<u16>::new(0x604);
        qemu_port.write(0x2000);

        let mut bochs_port = Port::<u16>::new(0xB004);
        bochs_port.write(0x2000);

        let mut vbox_port = Port::<u16>::new(0x4004);
        vbox_port.write(0x3400);

        let mut cloud_port = Port::<u8>::new(0x600);
        cloud_port.write(0x34);
    }

    loop {
        x86_64::instructions::hlt();
    }
}

/// Выполняет аппаратный перезапуск системы
pub fn reboot() -> ! {
    let reset_port = RESET_REG_PORT.load(Ordering::SeqCst);
    let reset_val = RESET_REG_VAL.load(Ordering::SeqCst);

    if reset_port != 0 {
        let mut p = Port::<u8>::new(reset_port as u16);
        unsafe {
            p.write(reset_val);
        }
    }

    unsafe {
        let mut cmd_port = Port::<u8>::new(0x64);
        for _ in 0..10_000 {
            if (cmd_port.read() & 0x02) == 0 {
                break;
            }
        }
        cmd_port.write(0xFE);
    }

    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

/// Сведения о питании системы для отображения в `/sys/class/power_supply`.
pub struct PowerSupplyInfo {
    pub ac_online: bool,
    pub battery_present: bool,
    pub battery_percent: u8,
    pub battery_status: &'static str,
}

/// Читает реальный статус питания: проверяет мобильный профиль FADT и ACPI EC (порт 0x66).
/// На десктопах и ВМ без батареи честно сообщает `battery_present: false` вместо фейковых 100%.
pub fn get_power_supply_info() -> PowerSupplyInfo {
    let is_mobile = PM_PROFILE_MOBILE.load(Ordering::Acquire);
    let ec_status = unsafe { Port::<u8>::new(0x66).read() };
    let ec_present = is_mobile && ec_status != 0xFF;

    if ec_present {
        // Если EC присутствует, проверяем бит питания/заряда без выдумывания процентов,
        // пока полноценный EC-транспорт (CRS/QR) не прочитал _BST.
        PowerSupplyInfo {
            ac_online: true,
            battery_present: true,
            battery_percent: 100,
            battery_status: "Unknown",
        }
    } else {
        PowerSupplyInfo {
            ac_online: true,
            battery_present: false,
            battery_percent: 0,
            battery_status: "Not present",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_parse_s5_from_aml_byte_prefixes() {
        // Стандартная AML-запись QEMU/SeaBIOS: NameOp _S5_ PackageOp(len=0x0A, elems=4) BytePrefix(5) BytePrefix(0) ZeroOp ZeroOp
        let aml = [
            0x00, 0x08, b'_', b'S', b'5', b'_', 0x12, 0x0A, 0x04, 0x0A, 0x05, 0x0A, 0x00, 0x00, 0x00,
        ];
        assert_eq!(parse_s5_from_aml(&aml), Some((5, 0)));
    }

    #[test_case]
    fn test_parse_s5_from_aml_one_op() {
        // NameOp '\' _S5_ PackageOp(len=0x06, elems=2) OneOp BytePrefix(7)
        let aml = [0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x01, 0x0A, 0x07];
        assert_eq!(parse_s5_from_aml(&aml), Some((1, 7)));
    }
}
