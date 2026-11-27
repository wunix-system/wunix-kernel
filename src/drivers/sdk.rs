use alloc::vec::Vec;
use spin::RwLock;
use wunix_drv::DriverDescriptor;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Контекст инициализации драйвера, передаваемый ядром в `probe`.
pub struct ProbeContext<'a> {
    pub dm: &'a mut DeviceManager,
    pub block_count: &'a mut usize,
}

/// Запись о декларативно зарегистрированном PCI-драйвере ядра.
/// Сторонние разработчики добавляют свой драйвер в таблицу без правок логики сканера PCI.
#[derive(Clone, Copy)]
pub struct PciDriverEntry {
    pub descriptor: &'static DriverDescriptor,
    pub probe: fn(&mut PciDevice, &mut ProbeContext<'_>) -> bool,
}

static DYNAMIC_PCI_DRIVERS: RwLock<Vec<PciDriverEntry>> = RwLock::new(Vec::new());

/// Динамически регистрирует дополнительный PCI-драйвер (в том числе из внешних модулей/крейтов).
pub fn register_pci_driver(entry: PciDriverEntry) {
    let mut list = DYNAMIC_PCI_DRIVERS.write();
    if !list.iter().any(|e| e.descriptor.name == entry.descriptor.name) {
        list.push(entry);
    }
}

/// Запускает probe по всем динамически зарегистрированным драйверам, чьи PCI ID подходят.
pub fn probe_dynamic_pci_drivers(dev: &mut PciDevice, ctx: &mut ProbeContext<'_>) -> bool {
    let entries: Vec<PciDriverEntry> = DYNAMIC_PCI_DRIVERS.read().clone();
    for entry in entries {
        let matched = entry.descriptor.pci_ids.iter().any(|id| {
            id.matches(
                dev.vendor_id,
                dev.device_id,
                dev.class_code,
                dev.subclass,
                dev.prog_if,
            )
        });
        if matched && (entry.probe)(dev, ctx) {
            if dev.driver_name.is_none() {
                dev.driver_name = Some(entry.descriptor.name);
            }
            return true;
        }
    }
    false
}

/// Возвращает паспорта всех динамически зарегистрированных драйверов.
pub fn dynamic_driver_descriptors() -> Vec<&'static DriverDescriptor> {
    DYNAMIC_PCI_DRIVERS
        .read()
        .iter()
        .map(|e| e.descriptor)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wunix_drv::{DriverCategory, PciMatchId};

    static TEST_IDS: &[PciMatchId] = &[PciMatchId::vendor_device(0x1234, 0xCAFE)];
    static TEST_DESC: DriverDescriptor = DriverDescriptor {
        name: "test_sdk_drv",
        version: "0.1.0",
        category: DriverCategory::Char,
        author: "Wunix Tests",
        license: "MIT",
        description_ru: "Тестовый SDK драйвер",
        description_en: "Test SDK driver",
        dependencies: "pci_core",
        pci_ids: TEST_IDS,
        usb_ids: &[],
    };

    fn test_probe(dev: &mut PciDevice, _ctx: &mut ProbeContext<'_>) -> bool {
        dev.driver_name = Some("test_sdk_drv");
        true
    }

    #[test_case]
    fn test_pci_driver_sdk_matching() {
        register_pci_driver(PciDriverEntry {
            descriptor: &TEST_DESC,
            probe: test_probe,
        });

        let mut dev = PciDevice {
            address: crate::drivers::pci::PciAddress::new(0, 1, 0),
            vendor_id: 0x1234,
            device_id: 0xCAFE,
            class_code: 0xFF,
            subclass: 0x00,
            prog_if: 0x00,
            revision_id: 0,
            header_type: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
            command: 0,
            status: 0,
            bars: [None; 6],
            subsystem_vendor_id: 0,
            subsystem_id: 0,
            driver_name: None,
        };

        let mut dm = DeviceManager::new();
        let mut blk = 0usize;
        let mut ctx = ProbeContext {
            dm: &mut dm,
            block_count: &mut blk,
        };

        assert!(probe_dynamic_pci_drivers(&mut dev, &mut ctx));
        assert_eq!(dev.driver_name, Some("test_sdk_drv"));
    }
}
