use alloc::vec::Vec;
use super::DriverInfo;
use crate::drivers::manager::DeviceManager;
use crate::i18n::t;

struct PciDriverSpec {
    pci_match: &'static str,
    mod_name: &'static str,
    desc_ru: &'static str,
    desc_en: &'static str,
    category: &'static str,
    size: usize,
}

const PCI_DRIVER_SPECS: &[PciDriverSpec] = &[
    PciDriverSpec {
        pci_match: "nvme",
        mod_name: "nvme",
        desc_ru: "Контроллер NVMe PCIe SSD (1.x..2.x)",
        desc_en: "NVM Express 1.x..2.x PCIe SSD Controller",
        category: "block",
        size: 32768,
    },
    PciDriverSpec {
        pci_match: "ahci",
        mod_name: "ahci",
        desc_ru: "Контроллер Serial ATA AHCI",
        desc_en: "Serial ATA AHCI 1.0..1.3.1 Controller",
        category: "block",
        size: 32768,
    },
    PciDriverSpec {
        pci_match: "ata_piix",
        mod_name: "ata_piix",
        desc_ru: "Контроллер Intel PIIX ATA/IDE",
        desc_en: "Intel PIIX ATA/IDE Controller",
        category: "block",
        size: 20480,
    },
    PciDriverSpec {
        pci_match: "sdhci",
        mod_name: "sdhci",
        desc_ru: "Хост-контроллер SD/eMMC (SDHCI)",
        desc_en: "SD Host Controller (SD/eMMC Reader)",
        category: "block",
        size: 28672,
    },
    PciDriverSpec {
        pci_match: "e1000",
        mod_name: "e1000",
        desc_ru: "Гигабитный Ethernet Intel PRO/1000 (82540EM/82574L)",
        desc_en: "Intel PRO/1000 Gigabit Ethernet (82540EM/82545EM/82574L)",
        category: "net",
        size: 36864,
    },
    PciDriverSpec {
        pci_match: "rtl8139",
        mod_name: "rtl8139",
        desc_ru: "Сетевой адаптер Realtek RTL8139 10/100",
        desc_en: "Realtek RTL8139 Fast Ethernet 10/100",
        category: "net",
        size: 24576,
    },
    PciDriverSpec {
        pci_match: "rtl8168",
        mod_name: "rtl8168",
        desc_ru: "Гигабитный адаптер Realtek RTL8168/8111 PCIe",
        desc_en: "Realtek RTL8168/8111 PCIe Gigabit Ethernet",
        category: "net",
        size: 28672,
    },
    PciDriverSpec {
        pci_match: "rtl8125",
        mod_name: "rtl8125",
        desc_ru: "Сетевой адаптер Realtek RTL8125 2.5GbE",
        desc_en: "Realtek RTL8125 2.5GbE Ethernet Adapter",
        category: "net",
        size: 32768,
    },
    PciDriverSpec {
        pci_match: "intel-ixgbe",
        mod_name: "ixgbe",
        desc_ru: "10GbE адаптер Intel 82599/X540/X550",
        desc_en: "Intel 82599/X540/X550 10 Gigabit Ethernet",
        category: "net",
        size: 40960,
    },
    PciDriverSpec {
        pci_match: "broadcom-tg3",
        mod_name: "tg3",
        desc_ru: "Гигабитный адаптер Broadcom NetXtreme Tigon3",
        desc_en: "Broadcom NetXtreme Tigon3 Gigabit Ethernet",
        category: "net",
        size: 36864,
    },
    PciDriverSpec {
        pci_match: "intel-iwlwifi",
        mod_name: "iwlwifi",
        desc_ru: "Беспроводной адаптер Intel Wi-Fi (iwlwifi)",
        desc_en: "Intel Wireless Wi-Fi 6/6E/7 (iwlwifi)",
        category: "net",
        size: 49152,
    },
    PciDriverSpec {
        pci_match: "xhci",
        mod_name: "xhci",
        desc_ru: "Хост-контроллер USB 3.x (xHCI)",
        desc_en: "eXtensible Host Controller Interface (USB 3.x)",
        category: "bus",
        size: 36864,
    },
    PciDriverSpec {
        pci_match: "ehci",
        mod_name: "ehci",
        desc_ru: "Хост-контроллер USB 2.0 (EHCI)",
        desc_en: "Enhanced Host Controller Interface (USB 2.0)",
        category: "bus",
        size: 24576,
    },
    PciDriverSpec {
        pci_match: "ohci",
        mod_name: "ohci",
        desc_ru: "Хост-контроллер USB 1.1 (OHCI)",
        desc_en: "Open Host Controller Interface (USB 1.1)",
        category: "bus",
        size: 20480,
    },
    PciDriverSpec {
        pci_match: "uhci",
        mod_name: "uhci",
        desc_ru: "Хост-контроллер USB 1.1 (UHCI)",
        desc_en: "Universal Host Controller Interface (USB 1.1)",
        category: "bus",
        size: 20480,
    },
    PciDriverSpec {
        pci_match: "snd_hda",
        mod_name: "snd_hda",
        desc_ru: "Звуковой контроллер Intel High Definition Audio",
        desc_en: "Intel High Definition Audio Controller",
        category: "sound",
        size: 28672,
    },
    PciDriverSpec {
        pci_match: "snd_ac97",
        mod_name: "snd_ac97",
        desc_ru: "Звуковой контроллер Intel ICH AC'97",
        desc_en: "Intel ICH AC'97 Audio Controller",
        category: "sound",
        size: 24576,
    },
    PciDriverSpec {
        pci_match: "i2c-designware",
        mod_name: "i2c_designware",
        desc_ru: "Контроллер шины I2C (Synopsys DesignWare / Intel LPSS)",
        desc_en: "Synopsys DesignWare / Intel LPSS I2C Bus Adapter",
        category: "bus",
        size: 20480,
    },
];

/// Добавляет драйверы реального оборудования шины PCI и периферийных интерфейсов.
pub fn append_pci_drivers(list: &mut Vec<DriverInfo>, dm: &DeviceManager) {
    let pci_devs = dm.pci_devices();
    if !pci_devs.is_empty() {
        list.push(DriverInfo {
            name: "pci_core",
            description: t(
                "Перечислитель шины PCI и менеджер устройств",
                "PCI Bus Enumeration & Device Manager (Config Mechanism 1)",
            ),
            category: "bus",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: pci_devs.len(),
            status: "Live",
            size: 24576,
            dependencies: "-",
        });
    }

    for spec in PCI_DRIVER_SPECS {
        let count = pci_devs
            .iter()
            .filter(|d| d.driver_name == Some(spec.pci_match))
            .count();
        if count > 0 {
            list.push(DriverInfo {
                name: spec.mod_name,
                description: t(spec.desc_ru, spec.desc_en),
                category: spec.category,
                version: env!("CARGO_PKG_VERSION"),
                author: "Wunix Kernel Team",
                license: "MIT",
                devices_count: count,
                status: "Live",
                size: spec.size,
                dependencies: "pci_core",
            });
        }
    }

    if crate::drivers::audio::sb16::is_present() {
        list.push(DriverInfo {
            name: "snd_sb16",
            description: t(
                "Звуковая карта Sound Blaster 16 DSP (ISA DMA)",
                "Creative Sound Blaster 16 DSP Audio",
            ),
            category: "sound",
            version: env!("CARGO_PKG_VERSION"),
            author: "Wunix Kernel Team",
            license: "MIT",
            devices_count: 1,
            status: "Live",
            size: 16384,
            dependencies: "-",
        });
    }
}
