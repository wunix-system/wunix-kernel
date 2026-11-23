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

    // 1. Обязательные базовые системные драйверы архитектуры x86_64
    system::append_system_drivers(&mut list);

    // 2. Драйверы шины PCI, контроллеров ввода-вывода и периферии
    with_device_manager(|dm| {
        pci_devs::append_pci_drivers(&mut list, dm);
        virt::append_virt_drivers(&mut list, dm);
    });

    list
}
