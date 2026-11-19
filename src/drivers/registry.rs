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

/// Динамически формирует реестр всех реально активных аппаратных драйверов ядра
pub fn list_drivers() -> Vec<DriverInfo> {
    let mut list = Vec::new();

    // 1. Обязательные системные драйверы платформы x86_64
    list.push(DriverInfo {
        name: "ps2_keyboard",
        description: "Standard PS/2 Keyboard Driver",
        category: "input",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 12288,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "uart16550",
        description: "UART 16550 Serial COM Port Driver",
        category: "char",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 8192,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "limine_fb",
        description: "Limine Linear Framebuffer Display Driver",
        category: "display",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 16384,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "rtc_cmos",
        description: "Motorola MC146818 CMOS Real-Time Clock",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "pit8254",
        description: "Intel 8254 Programmable Interval Timer",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    list.push(DriverInfo {
        name: "i8259_pic",
        description: "Dual 8259 Programmable Interrupt Controller",
        category: "system",
        version: "1.0.0",
        author: "Wunix Kernel Team",
        license: "MIT/Apache-2.0",
        devices_count: 1,
        status: "Live",
        size: 4096,
        dependencies: "-",
    });

    // 2. Динамический опрос активных шинных устройств через DeviceManager
    with_device_manager(|dm| {
        let pci_devs = dm.pci_devices();
        if !pci_devs.is_empty() {
            list.push(DriverInfo {
                name: "pci_core",
                description: "PCI Bus Enumeration and Device Manager",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: pci_devs.len(),
                status: "Live",
                size: 24576,
                dependencies: "-",
            });
        }

        // Подсчет конкретных драйверов по PCI устройствам
        let count_pci = |drv_name: &str| -> usize {
            pci_devs.iter().filter(|d| d.driver_name == Some(drv_name)).count()
        };

        // Драйверы накопителей
        let nvme_c = count_pci("nvme");
        if nvme_c > 0 {
            list.push(DriverInfo {
                name: "nvme",
                description: "NVM Express 1.x..2.x PCIe SSD Controller",
                category: "block",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: nvme_c,
                status: "Live",
                size: 32768,
                dependencies: "pci_core",
            });
        }

        let ahci_c = count_pci("ahci");
        if ahci_c > 0 {
            list.push(DriverInfo {
                name: "ahci",
                description: "Serial ATA AHCI 1.0..1.3.1 Controller",
                category: "block",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ahci_c,
                status: "Live",
                size: 32768,
                dependencies: "pci_core",
            });
        }

        let ata_c = count_pci("ata_piix");
        if ata_c > 0 {
            list.push(DriverInfo {
                name: "ata_piix",
                description: "Intel PIIX ATA/IDE Controller",
                category: "block",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ata_c,
                status: "Live",
                size: 20480,
                dependencies: "pci_core",
            });
        }

        // Драйверы сетевых адаптеров
        let e1000_c = count_pci("e1000");
        if e1000_c > 0 {
            list.push(DriverInfo {
                name: "e1000",
                description: "Intel PRO/1000 / e1000e Gigabit Ethernet",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: e1000_c,
                status: "Live",
                size: 36864,
                dependencies: "pci_core",
            });
        }

        let rtl8139_c = count_pci("rtl8139");
        if rtl8139_c > 0 {
            list.push(DriverInfo {
                name: "rtl8139",
                description: "Realtek RTL8139 Fast Ethernet 10/100",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: rtl8139_c,
                status: "Live",
                size: 24576,
                dependencies: "pci_core",
            });
        }

        let rtl8168_c = count_pci("rtl8168");
        if rtl8168_c > 0 {
            list.push(DriverInfo {
                name: "rtl8168",
                description: "Realtek RTL8168/8111 PCIe Gigabit Ethernet",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: rtl8168_c,
                status: "Live",
                size: 28672,
                dependencies: "pci_core",
            });
        }

        let rtl8125_c = count_pci("rtl8125");
        if rtl8125_c > 0 {
            list.push(DriverInfo {
                name: "rtl8125",
                description: "Realtek RTL8125 2.5GbE Ethernet Adapter",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: rtl8125_c,
                status: "Live",
                size: 32768,
                dependencies: "pci_core",
            });
        }

        let ixgbe_c = count_pci("intel-ixgbe");
        if ixgbe_c > 0 {
            list.push(DriverInfo {
                name: "ixgbe",
                description: "Intel 82599/X540/X550 10 Gigabit Ethernet",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ixgbe_c,
                status: "Live",
                size: 40960,
                dependencies: "pci_core",
            });
        }

        let tg3_c = count_pci("broadcom-tg3");
        if tg3_c > 0 {
            list.push(DriverInfo {
                name: "tg3",
                description: "Broadcom NetXtreme Gigabit Ethernet",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: tg3_c,
                status: "Live",
                size: 36864,
                dependencies: "pci_core",
            });
        }

        let iwl_c = count_pci("intel-iwlwifi");
        if iwl_c > 0 {
            list.push(DriverInfo {
                name: "iwlwifi",
                description: "Intel Wireless Wi-Fi 6/6E/7 (AX200/AX210)",
                category: "net",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: iwl_c,
                status: "Live",
                size: 49152,
                dependencies: "pci_core",
            });
        }

        let sdhci_c = count_pci("sdhci");
        if sdhci_c > 0 {
            list.push(DriverInfo {
                name: "sdhci",
                description: "SD Host Controller (SD/eMMC Card Reader)",
                category: "block",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: sdhci_c,
                status: "Live",
                size: 28672,
                dependencies: "pci_core",
            });
        }

        // Драйверы шины USB
        let xhci_c = count_pci("xhci");
        if xhci_c > 0 {
            list.push(DriverInfo {
                name: "xhci",
                description: "eXtensible Host Controller Interface (USB 3.x)",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: xhci_c,
                status: "Live",
                size: 36864,
                dependencies: "pci_core",
            });
        }

        let ehci_c = count_pci("ehci");
        if ehci_c > 0 {
            list.push(DriverInfo {
                name: "ehci",
                description: "Enhanced Host Controller Interface (USB 2.0)",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ehci_c,
                status: "Live",
                size: 24576,
                dependencies: "pci_core",
            });
        }

        let ohci_c = count_pci("ohci");
        if ohci_c > 0 {
            list.push(DriverInfo {
                name: "ohci",
                description: "Open Host Controller Interface (USB 1.1)",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ohci_c,
                status: "Live",
                size: 20480,
                dependencies: "pci_core",
            });
        }

        let uhci_c = count_pci("uhci");
        if uhci_c > 0 {
            list.push(DriverInfo {
                name: "uhci",
                description: "Universal Host Controller Interface (USB 1.1)",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: uhci_c,
                status: "Live",
                size: 20480,
                dependencies: "pci_core",
            });
        }

        // Аудиодрайверы
        let hda_c = count_pci("snd_hda");
        if hda_c > 0 {
            list.push(DriverInfo {
                name: "snd_hda",
                description: "Intel High Definition Audio Controller",
                category: "sound",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: hda_c,
                status: "Live",
                size: 28672,
                dependencies: "pci_core",
            });
        }

        let ac97_c = count_pci("snd_ac97");
        if ac97_c > 0 {
            list.push(DriverInfo {
                name: "snd_ac97",
                description: "Intel ICH AC'97 Audio Controller",
                category: "sound",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: ac97_c,
                status: "Live",
                size: 24576,
                dependencies: "pci_core",
            });
        }

        if dm.audio_device_count() > 0 && hda_c == 0 && ac97_c == 0 {
            list.push(DriverInfo {
                name: "snd_sb16",
                description: "Creative Sound Blaster 16 DSP Audio",
                category: "sound",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: 1,
                status: "Live",
                size: 16384,
                dependencies: "-",
            });
        }

        // Драйверы VirtIO
        let virtio_c = pci_devs.iter().filter(|d| d.vendor_id == 0x1AF4).count();
        if virtio_c > 0 {
            list.push(DriverInfo {
                name: "virtio_pci",
                description: "VirtIO Modern/Legacy PCI Transport",
                category: "bus",
                version: "1.0.0",
                author: "Wunix Kernel Team",
                license: "MIT/Apache-2.0",
                devices_count: virtio_c,
                status: "Live",
                size: 20480,
                dependencies: "pci_core",
            });
        }
    });

    list
}
