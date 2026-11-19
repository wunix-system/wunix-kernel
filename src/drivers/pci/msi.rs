use super::{config, PciAddress, PciDevice};

const PCI_CAPABILITY_LIST_PTR: u8 = 0x34;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_STATUS_OFFSET: u8 = 0x06;

const CAP_ID_MSI: u8 = 0x05;
pub const CAP_ID_MSIX: u8 = 0x11;

/// Ищет смещение расширенной возможности (Capability) в конфигурационном пространстве PCI
pub fn find_capability(addr: PciAddress, cap_id: u8) -> Option<u8> {
    let status = config::read_u16(addr, PCI_STATUS_OFFSET);
    if (status & PCI_STATUS_CAP_LIST) == 0 {
        return None;
    }

    let mut cap_ptr = config::read_u8(addr, PCI_CAPABILITY_LIST_PTR) & !0x03;
    let mut visited = 0;

    while cap_ptr != 0 && visited < 48 {
        let id = config::read_u8(addr, cap_ptr);
        if id == cap_id {
            return Some(cap_ptr);
        }
        let next = config::read_u8(addr, cap_ptr + 1) & !0x03;
        if next == cap_ptr {
            break;
        }
        cap_ptr = next;
        visited += 1;
    }

    None
}

/// Настраивает Message Signaled Interrupts (MSI) для PCI-устройства
pub fn enable_msi(dev: &PciDevice, vector: u8, dest_apic_id: u8) -> Result<(), &'static str> {
    let cap_offset = find_capability(dev.address, CAP_ID_MSI)
        .ok_or("msi: device does not support MSI capability")?;

    let msg_ctrl = config::read_u16(dev.address, cap_offset + 2);
    let is_64bit = (msg_ctrl & (1 << 7)) != 0;

    // Адрес сообщения MSI для архитектуры x86:
    // База: 0xFEE00000 | (Destination APIC ID << 12) | Redirection Hint (0) | Destination Mode (Physical = 0)
    let msg_addr = 0xFEE0_0000 | ((dest_apic_id as u32) << 12);
    config::write_u32(dev.address, cap_offset + 4, msg_addr);

    let data_offset = if is_64bit {
        config::write_u32(dev.address, cap_offset + 8, 0); // Upper 32 bits = 0
        cap_offset + 12
    } else {
        cap_offset + 8
    };

    // Данные сообщения: вектор прерывания (биты 0..7), Edge-triggered (бит 15 = 0)
    config::write_u16(dev.address, data_offset, vector as u16);

    // Включаем MSI (бит 0 в Message Control)
    let new_ctrl = msg_ctrl | 1;
    config::write_u16(dev.address, cap_offset + 2, new_ctrl);

    crate::klog!(
        "pci: enabled MSI for device {:?} (vector: {}, APIC ID: {})",
        dev.address,
        vector,
        dest_apic_id
    );

    Ok(())
}
