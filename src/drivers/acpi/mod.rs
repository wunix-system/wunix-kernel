pub mod power;
pub mod tables;

use alloc::vec::Vec;
use spin::Mutex;
use self::tables::{AcpiHeader, Fadt, MadtHeader, MadtIoApic, MadtIso, Rsdp};

#[derive(Debug, Clone)]
pub struct MadtInfo {
    pub local_apic_phys: u64,
    pub io_apics: Vec<MadtIoApic>,
    pub interrupt_source_overrides: Vec<MadtIso>,
}

static MADT_INFO: Mutex<Option<MadtInfo>> = Mutex::new(None);
static RSDP_OVERRIDE: Mutex<Option<u64>> = Mutex::new(None);

pub fn set_rsdp_address(addr: u64) {
    *RSDP_OVERRIDE.lock() = Some(addr);
}

fn map_table(phys: u64, size: usize) -> Option<u64> {
    crate::memory::map_kernel_mmio(phys, size).ok().map(|v| v.as_u64())
}

/// Инициализирует подсистему ACPI: обнаруживает таблицы FADT и MADT
pub fn init() {
    let rsdp_raw = match *RSDP_OVERRIDE.lock() {
        Some(p) => p,
        None => {
            crate::klog!("acpi: RSDP address not provided by bootloader");
            return;
        }
    };

    let rsdp_virt = if rsdp_raw >= 0xFFFF_8000_0000_0000 {
        rsdp_raw
    } else {
        match map_table(rsdp_raw, 64) {
            Some(v) => v,
            None => return,
        }
    };

    let rsdp = unsafe { &*(rsdp_virt as *const Rsdp) };
    if !rsdp.is_valid() {
        crate::klog!("acpi: invalid RSDP structure at {:#x}", rsdp_raw);
        return;
    }
    crate::klog!("acpi: RSDP registered (revision {})", rsdp.revision);

    let mut fadt_found = false;
    let mut madt_found = false;

    // Читаем XSDT (64-битные указатели) или RSDT (32-битные указатели)
    if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
        let xsdt_virt = match map_table(rsdp.xsdt_address, 4096) {
            Some(v) => v,
            None => return,
        };
        let xsdt = unsafe { &*(xsdt_virt as *const AcpiHeader) };
        if xsdt.is_valid() && &xsdt.signature == b"XSDT" {
            let entries_count = (xsdt.length as usize - core::mem::size_of::<AcpiHeader>()) / 8;
            let entries_ptr = (xsdt_virt + core::mem::size_of::<AcpiHeader>() as u64) as *const u64;
            for i in 0..entries_count {
                let table_phys = unsafe { *entries_ptr.add(i) };
                process_table(table_phys, &mut fadt_found, &mut madt_found);
            }
        }
    } else if rsdp.rsdt_address != 0 {
        let rsdt_virt = match map_table(rsdp.rsdt_address as u64, 4096) {
            Some(v) => v,
            None => return,
        };
        let rsdt = unsafe { &*(rsdt_virt as *const AcpiHeader) };
        if rsdt.is_valid() && &rsdt.signature == b"RSDT" {
            let entries_count = (rsdt.length as usize - core::mem::size_of::<AcpiHeader>()) / 4;
            let entries_ptr = (rsdt_virt + core::mem::size_of::<AcpiHeader>() as u64) as *const u32;
            for i in 0..entries_count {
                let table_phys = unsafe { *entries_ptr.add(i) } as u64;
                process_table(table_phys, &mut fadt_found, &mut madt_found);
            }
        }
    }
}

fn process_table(table_phys: u64, fadt_found: &mut bool, madt_found: &mut bool) {
    let table_virt = match map_table(table_phys, 4096) {
        Some(v) => v,
        None => return,
    };
    let header = unsafe { &*(table_virt as *const AcpiHeader) };
    if !header.is_valid() {
        return;
    }

    if &header.signature == b"FACP" && !*fadt_found {
        let fadt = unsafe { &*(table_virt as *const Fadt) };
        let pm1a = fadt.pm1a_control_block;
        power::init_power_from_fadt(fadt);
        crate::klog!("acpi: FADT registered (PM1a port: {:#x})", pm1a);
        *fadt_found = true;
    } else if &header.signature == b"APIC" && !*madt_found {
        parse_madt(table_phys, table_virt);
        *madt_found = true;
    }
}

fn parse_madt(_madt_phys: u64, madt_virt: u64) {
    let madt = unsafe { &*(madt_virt as *const MadtHeader) };
    let total_len = madt.header.length as usize;
    let header_len = core::mem::size_of::<MadtHeader>();
    let lapic_phys = madt.local_apic_address as u64;

    let mut io_apics = Vec::new();
    let mut isos = Vec::new();

    let mut offset = header_len;
    while offset + 2 <= total_len {
        let entry_ptr = (madt_virt + offset as u64) as *const u8;
        let entry_type = unsafe { *entry_ptr };
        let entry_len = unsafe { *entry_ptr.add(1) } as usize;

        if entry_len < 2 || offset + entry_len > total_len {
            break;
        }

        match entry_type {
            1 => {
                if entry_len >= core::mem::size_of::<MadtIoApic>() {
                    let io_apic = unsafe { core::ptr::read_unaligned(entry_ptr as *const MadtIoApic) };
                    io_apics.push(io_apic);
                }
            }
            2 => {
                if entry_len >= core::mem::size_of::<MadtIso>() {
                    let iso = unsafe { core::ptr::read_unaligned(entry_ptr as *const MadtIso) };
                    isos.push(iso);
                }
            }
            _ => {}
        }

        offset += entry_len;
    }

    crate::klog!(
        "acpi: MADT registered (Local APIC: {:#x}, IO-APICs: {}, ISOs: {})",
        lapic_phys,
        io_apics.len(),
        isos.len()
    );

    *MADT_INFO.lock() = Some(MadtInfo {
        local_apic_phys: lapic_phys,
        io_apics,
        interrupt_source_overrides: isos,
    });
}

pub fn get_madt_info() -> Option<MadtInfo> {
    MADT_INFO.lock().clone()
}
