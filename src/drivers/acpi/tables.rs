use core::mem;

/// Стандартный заголовок любой таблицы ACPI (SDT)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct AcpiHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl AcpiHeader {
    /// Проверяет контрольную сумму таблицы: сумма всех байтов таблицы по модулю 256 должна быть равна 0
    pub fn is_valid(&self) -> bool {
        let ptr = self as *const Self as *const u8;
        let len = self.length as usize;
        if len < mem::size_of::<AcpiHeader>() || len > 1024 * 1024 {
            return false;
        }
        let mut sum: u8 = 0;
        for i in 0..len {
            sum = sum.wrapping_add(unsafe { *ptr.add(i) });
        }
        sum == 0
    }
}

/// Корневой указатель на структуры описания системы (RSDP ACPI 1.0 / 2.0+)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

impl Rsdp {
    pub fn is_valid(&self) -> bool {
        if &self.signature != b"RSD PTR " {
            return false;
        }
        let ptr = self as *const Self as *const u8;
        let mut sum: u8 = 0;
        for i in 0..20 {
            sum = sum.wrapping_add(unsafe { *ptr.add(i) });
        }
        if sum != 0 {
            return false;
        }
        if self.revision >= 2 {
            let mut ext_sum: u8 = 0;
            let len = self.length as usize;
            if len < 36 || len > 128 {
                return false;
            }
            for i in 0..len {
                ext_sum = ext_sum.wrapping_add(unsafe { *ptr.add(i) });
            }
            return ext_sum == 0;
        }
        true
    }
}

/// Универсальная структура адреса ACPI (Generic Address Structure - GAS)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct GenericAddress {
    pub address_space: u8, // 0 = System Memory (MMIO), 1 = System I/O Ports
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// Таблица FADT (Fixed ACPI Description Table) — регистры питания, таймера и перезагрузки
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Fadt {
    pub header: AcpiHeader,
    pub firmware_ctrl: u32,
    pub dsdt: u32,
    pub reserved1: u8,
    pub preferred_pm_profile: u8,
    pub sci_interrupt: u16,
    pub smi_command_port: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub s4bios_req: u8,
    pub pstate_control: u8,
    pub pm1a_event_block: u32,
    pub pm1b_event_block: u32,
    pub pm1a_control_block: u32,
    pub pm1b_control_block: u32,
    pub pm2_control_block: u32,
    pub pm_timer_block: u32,
    pub gpe0_block: u32,
    pub gpe1_block: u32,
    pub pm1_event_length: u8,
    pub pm1_control_length: u8,
    pub pm2_control_length: u8,
    pub pm_timer_length: u8,
    pub gpe0_length: u8,
    pub gpe1_length: u8,
    pub gpe1_base: u8,
    pub cstate_control: u8,
    pub worst_c2_latency: u16,
    pub worst_c3_latency: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alarm: u8,
    pub month_alarm: u8,
    pub century: u8,
    pub iapc_boot_arch: u16,
    pub reserved2: u8,
    pub flags: u32,
    pub reset_reg: GenericAddress,
    pub reset_value: u8,
    pub arm_boot_arch: u16,
    pub fadt_minor_version: u8,
    pub x_firmware_ctrl: u64,
    pub x_dsdt: u64,
    pub x_pm1a_event_block: GenericAddress,
    pub x_pm1b_event_block: GenericAddress,
    pub x_pm1a_control_block: GenericAddress,
    pub x_pm1b_control_block: GenericAddress,
}

/// Заголовок таблицы MADT (Multiple APIC Description Table)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MadtHeader {
    pub header: AcpiHeader,
    pub local_apic_address: u32,
    pub flags: u32,
}

/// Запись об одном I/O APIC в MADT
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MadtIoApic {
    pub entry_type: u8,
    pub length: u8,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// Запись о переопределении источника прерывания (ISO) в MADT
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MadtIso {
    pub entry_type: u8,
    pub length: u8,
    pub bus: u8,
    pub source: u8,
    pub global_system_interrupt: u32,
    pub flags: u16,
}
