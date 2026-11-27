pub mod pci_devs;
pub mod system;
pub mod virt;

use alloc::vec::Vec;
use crate::drivers::manager::with_device_manager;

/// Метаданные загруженного драйвера ядра Wunix
#[derive(Debug, Clone)]
pub struct DriverInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub version: &'static str,
    pub author: &'static str,
    pub license: &'static str,
    pub devices_count: usize,
    pub status: &'static str,
    pub size: usize,
    pub dependencies: &'static str,
}

/// Динамически формирует реестр всех реально активных аппаратных и виртуальных драйверов ядра
pub fn list_drivers() -> Vec<DriverInfo> {
    let mut list = Vec::new();
    let is_ru = crate::i18n::current_language() == crate::i18n::Language::Russian;

    // 1. Базовые системные драйверы архитектуры x86_64 (только реально обнаруженные)
    system::append_system_drivers(&mut list);

    // 2. Драйверы шины PCI, контроллеров ввода-вывода и периферии
    with_device_manager(|dm| {
        pci_devs::append_pci_drivers(&mut list, dm);
        virt::append_virt_drivers(&mut list, dm);
    });

    // 3. Драйверы, зарегистрированные через декларативный SDK
    for desc in crate::drivers::sdk::dynamic_driver_descriptors() {
        let count = with_device_manager(|dm| {
            dm.pci_devices()
                .iter()
                .filter(|d| d.driver_name == Some(desc.name))
                .count()
        })
        .unwrap_or(0);
        if count > 0 && !list.iter().any(|info| info.name == desc.name) {
            list.push(DriverInfo {
                name: desc.name,
                description: if is_ru {
                    desc.description_ru
                } else {
                    desc.description_en
                },
                category: desc.category.as_str(),
                version: desc.version,
                author: desc.author,
                license: desc.license,
                devices_count: count,
                status: "Live",
                size: 8192,
                dependencies: desc.dependencies,
            });
        }
    }

    list
}
