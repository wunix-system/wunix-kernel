use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;

use super::manager::DeviceManager;
use super::pci;
use super::sdk::{probe_dynamic_pci_drivers, ProbeContext};
use super::traits::{BlockDevice, NetDevice};
use super::virt::{self, HypervisorType};
use super::virtio;
use crate::i18n::{current_language, t, Language};
use crate::kboot;

/// Сканирует шину PCI, инициализирует контроллеры и интеграции сред виртуализации.
pub(super) fn init_hardware(dm: &mut DeviceManager) {
    super::acpi::init();
    crate::arch::x86_64::apic::init();
    super::tpm::init();
    if let Some(mouse) = super::input::ps2_mouse::init() {
        crate::arch::x86_64::interrupts::unmask_isa_irq(12);
        dm.register_input_device(mouse);
        kboot!(
            "pms0",
            "{}",
            t("мышь PS/2 (IRQ12)", "PS/2 mouse (IRQ12)")
        );
    }

    let hv = virt::init();
    let lang_ru = current_language() == Language::Russian;
    kboot!(
        "vmm0",
        "{} — {}",
        t("среда выполнения", "platform"),
        hv.display_name(lang_ru)
    );

    kboot!("pci0", "{}", t("<шина PCI>", "<PCI bus>"));
    let mut pci_list = pci::scan::scan_all_busses();
    let mut block_count = 0usize;

    for dev in &mut pci_list {
        probe_pci_device(dm, dev, &mut block_count);
        log_pci_device(dev);
    }

    dm.pci_devices = pci_list;
    init_hypervisor_integrations(dm, hv);
    super::audio::init_legacy(dm);

    kboot!(
        "pci0",
        "{}: {} ({}: {}, {}: {}, {}: {}, {}: {}, {}: {})",
        t("обнаружено устройств", "devices found"),
        dm.pci_devices.len(),
        t("дисков", "disks"),
        dm.block_devices.len(),
        t("сетей", "net"),
        dm.net_devices.len(),
        t("дисплеев", "display"),
        dm.display_devices.len(),
        t("ввода", "input"),
        dm.input_devices.len(),
        t("аудио", "audio"),
        dm.audio_devices.len()
    );
}

fn probe_pci_device(dm: &mut DeviceManager, dev: &mut pci::PciDevice, block_count: &mut usize) {
    let mut ctx = ProbeContext { dm, block_count };
    if probe_dynamic_pci_drivers(dev, &mut ctx) {
        return;
    }

    if dev.vendor_id == virtio::VIRTIO_PCI_VENDOR_ID {
        probe_virtio(dm, dev, block_count);
    } else if dev.class_code == 0x01 && dev.subclass == 0x01 {
        probe_ide(dm, dev);
    } else if super::ahci::probe_pci(dev, dm, block_count) {
        // Контроллер SATA / AHCI обработал устройство
    } else if super::nvme::probe_pci(dev, dm) {
        // Высокоскоростной SSD накопитель NVMe
    } else if virt::vbox::probe_pci(dev, dm) {
        dev.driver_name = Some(if dev.device_id == 0xBEEF {
            "vboxvideo"
        } else {
            "vboxguest"
        });
    } else if virt::vmware::probe_pci(dev, dm) {
        dev.driver_name = Some(match dev.device_id {
            0x0405 => "vmw_svga",
            0x07B0 => "vmxnet3",
            0x07C0 => "pvscsi",
            _ => "vmware",
        });
    } else if virt::hyperv::probe_pci(dev, dm) {
        dev.driver_name = Some("hyperv_fb");
    } else if virt::qemu::probe_pci(dev, dm) {
        dev.driver_name = Some("bochs-vga");
    } else if virt::xen::platform::probe_pci(dev) {
        dev.driver_name = Some("xen-platform");
    } else if super::net::probe_pci(dev, dm) {
        // Сетевые контроллеры Realtek / Intel / Broadcom
    } else if super::usb::probe_pci(dev, dm, block_count) {
        // Хост-контроллеры шины USB (xHCI, EHCI, OHCI, UHCI)
    } else if dev.class_code == 0x08 && dev.subclass == 0x05 {
        if super::sdhci::probe(dev, dm).is_ok() {
            dev.driver_name = Some("sdhci");
        }
    } else if super::audio::probe_pci(dev, dm) {
        // Звуковые контроллеры Intel HDA и AC'97
    } else if super::i2c::probe_pci(dev, dm) {
        // Контроллеры шины I2C и тачпады I2C-HID
    } else if let Some(nic) = super::e1000::probe_pci(dev) {
        dev.driver_name = Some("e1000");
        let mac = nic.mac_address();
        kboot!(
            "e1000",
            "{} eth{} (Intel PRO/1000, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            t("зарегистрирован интерфейс", "registered interface"),
            dm.net_devices.len(),
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
        dm.register_net_device(Arc::new(Mutex::new(nic)));
    }
}

fn probe_ide(dm: &mut DeviceManager, dev: &mut pci::PciDevice) {
    dev.driver_name = Some("ata_piix");
    dev.enable_io_space();
    dev.enable_bus_mastering();

    let channels = [
        (0x1F0u16, false, "hda"),
        (0x1F0u16, true, "hdb"),
        (0x170u16, false, "hdc"),
        (0x170u16, true, "hdd"),
    ];

    for (port, is_slave, name) in channels {
        if let Some(ata) = super::ata::AtaDevice::probe(port, is_slave) {
            let cap_mb = (ata.sector_count() * 512) / (1024 * 1024);
            kboot!(
                "ata",
                "{} /dev/{} ({} {}, {} {})",
                t("обнаружен диск", "registered disk"),
                name,
                cap_mb,
                t("МБ", "MiB"),
                ata.sector_count(),
                t("секторов", "sectors")
            );
            dm.register_block_device(String::from(name), Arc::new(Mutex::new(ata)));
        }
    }
}

fn probe_virtio(dm: &mut DeviceManager, dev: &mut pci::PciDevice, block_count: &mut usize) {
    let vtype = virtio::VirtioDeviceType::from_pci(dev.device_id, dev.subsystem_id);
    dev.driver_name = Some(match vtype {
        virtio::VirtioDeviceType::Block => "virtio_blk",
        virtio::VirtioDeviceType::Net => "virtio_net",
        virtio::VirtioDeviceType::Gpu => "virtio_gpu",
        virtio::VirtioDeviceType::Input => "virtio_input",
        virtio::VirtioDeviceType::Scsi => "virtio_scsi",
        virtio::VirtioDeviceType::Entropy => "virtio_rng",
        virtio::VirtioDeviceType::Console => "virtio_console",
        virtio::VirtioDeviceType::Balloon => "virtio_balloon",
        _ => "virtio_pci",
    });

    let transport = match virtio::transport::create_transport_from_pci(dev) {
        Ok(t) => t,
        Err(_) => return,
    };

    match vtype {
        virtio::VirtioDeviceType::Block => {
            if let Ok(blk) = virtio::blk::VirtioBlock::new(transport) {
                let letter = (b'a' + (*block_count as u8).min(25)) as char;
                let name = format!("vd{}", letter);
                let sectors = blk.sector_count();
                let cap_mb = (sectors * blk.sector_size() as u64) / (1024 * 1024);
                kboot!(
                    "virtio-blk",
                    "{} /dev/{} ({} {}, {} {})",
                    t("инициализирован диск", "initialized disk"),
                    name,
                    cap_mb,
                    t("МБ", "MiB"),
                    sectors,
                    t("секторов", "sectors")
                );
                dm.register_block_device(name, Arc::new(Mutex::new(blk)));
                *block_count += 1;
            }
        }
        virtio::VirtioDeviceType::Scsi => {
            if let Ok(scsi) = virtio::scsi::VirtioScsi::new(transport) {
                let letter = (b'a' + (*block_count as u8).min(25)) as char;
                let name = format!("sd{}", letter);
                let sectors = scsi.sector_count();
                let cap_mb = (sectors * scsi.sector_size() as u64) / (1024 * 1024);
                kboot!(
                    "virtio-scsi",
                    "{} /dev/{} ({} {})",
                    t("инициализирован SCSI диск", "initialized SCSI disk"),
                    name,
                    cap_mb,
                    t("МБ", "MiB")
                );
                dm.register_block_device(name, Arc::new(Mutex::new(scsi)));
                *block_count += 1;
            }
        }
        virtio::VirtioDeviceType::Net => {
            if let Ok(net) = virtio::net::VirtioNet::new(transport) {
                let mac = net.mac_address();
                kboot!(
                    "virtio-net",
                    "{} eth{} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, RX/TX)",
                    t("сетевой адаптер", "network adapter"),
                    dm.net_devices.len(),
                    mac[0],
                    mac[1],
                    mac[2],
                    mac[3],
                    mac[4],
                    mac[5]
                );
                dm.register_net_device(Arc::new(Mutex::new(net)));
            }
        }
        virtio::VirtioDeviceType::Input => {
            if let Ok(input) = virtio::input::VirtioInput::new(transport) {
                kboot!(
                    "virtio-input",
                    "{}",
                    t("устройство ввода зарегистрировано", "input device registered")
                );
                dm.register_input_device(Arc::new(Mutex::new(input)));
            }
        }
        virtio::VirtioDeviceType::Gpu => {
            let (w, h) = super::display::screen_dimensions().unwrap_or((1024, 768));
            if let Ok(gpu) = virtio::gpu::VirtioGpu::new(transport, w, h) {
                kboot!(
                    "virtio-gpu",
                    "{} ({}x{})",
                    t("графический 2D-адаптер инициализирован", "2D graphics adapter initialized"),
                    w,
                    h
                );
                dm.register_display_device(Arc::new(Mutex::new(gpu)));
            }
        }
        virtio::VirtioDeviceType::Entropy => {
            if let Ok(rng) = virtio::rng::VirtioRng::new(transport) {
                *virtio::rng::VIRTIO_RNG.lock() = Some(rng);
                kboot!(
                    "virtio-rng",
                    "{}",
                    t("активирован генератор энтропии", "hardware entropy device enabled")
                );
            }
        }
        virtio::VirtioDeviceType::Console => {
            if let Ok(cons) = virtio::console::VirtioConsole::new(transport) {
                dm.register_console_device(Arc::new(Mutex::new(cons)));
                kboot!(
                    "virtio-console",
                    "{}",
                    t("зарегистрирована консоль /dev/hvc0", "registered console /dev/hvc0")
                );
            }
        }
        virtio::VirtioDeviceType::Balloon => {
            if let Ok(balloon) = virtio::balloon::VirtioBalloon::new(transport) {
                dm.register_balloon_device(Arc::new(Mutex::new(balloon)));
                kboot!(
                    "virtio-balloon",
                    "{}",
                    t("драйвер управления памятью активен", "memory balloon driver active")
                );
            }
        }
        _ => {}
    }
}

fn init_hypervisor_integrations(dm: &mut DeviceManager, hv: HypervisorType) {
    match hv {
        HypervisorType::VirtualBox => virt::vbox::init_integrations(dm),
        HypervisorType::Kvm | HypervisorType::Qemu => virt::qemu::init_integrations(dm),
        HypervisorType::VMware => virt::vmware::init_integrations(dm),
        HypervisorType::HyperV => virt::hyperv::init_synthetic_drivers(dm),
        HypervisorType::Xen => virt::xen::init_xen_drivers(dm),
        _ => {}
    }
}

fn log_pci_device(dev: &pci::PciDevice) {
    let vendor = crate::fs::procfs::pci_vendor_name(dev.vendor_id);
    let dev_name = crate::fs::procfs::pci_device_name(dev.vendor_id, dev.device_id);
    let label = if dev_name == "Generic Device" {
        pci::PciClass::name(dev.class_code, dev.subclass)
    } else {
        dev_name
    };
    let drv = dev
        .driver_name
        .unwrap_or_else(|| crate::fs::procfs::pci_driver_name(dev.vendor_id, dev.device_id));
    kboot!(
        "",
        "pci0:{:02x}:{:02x}.{}: {} {} ({})",
        dev.address.bus,
        dev.address.device,
        dev.address.function,
        vendor,
        label,
        drv
    );
}
