use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::drivers::pci;
use crate::drivers::traits::{AudioDevice, BlockDevice, DisplayDevice, InputDevice, NetDevice};
use crate::drivers::virt::{self, HypervisorType};
use crate::drivers::virtio;
use crate::i18n::{current_language, t, Language};
use crate::kboot;

/// Реестр устройств ядра Wunix (DeviceManager)
pub struct DeviceManager {
    pub(crate) pci_devices: Vec<pci::PciDevice>,
    pub(crate) block_devices: BTreeMap<String, Arc<Mutex<dyn BlockDevice>>>,
    pub(crate) net_devices: Vec<Arc<Mutex<dyn NetDevice>>>,
    pub(crate) input_devices: Vec<Arc<Mutex<dyn InputDevice>>>,
    pub(crate) display_devices: Vec<Arc<Mutex<dyn DisplayDevice>>>,
    pub(crate) audio_devices: Vec<Arc<Mutex<dyn AudioDevice>>>,
}

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            pci_devices: Vec::new(),
            block_devices: BTreeMap::new(),
            net_devices: Vec::new(),
            input_devices: Vec::new(),
            display_devices: Vec::new(),
            audio_devices: Vec::new(),
        }
    }

    /// Регистрирует блочный накопитель по имени (например, "vda", "sda")
    pub fn register_block_device(&mut self, name: String, dev: Arc<Mutex<dyn BlockDevice>>) {
        self.block_devices.insert(name, dev);
    }

    /// Регистрирует сетевой адаптер
    pub fn register_net_device(&mut self, dev: Arc<Mutex<dyn NetDevice>>) {
        self.net_devices.push(dev);
    }

    /// Регистрирует устройство ввода
    pub fn register_input_device(&mut self, dev: Arc<Mutex<dyn InputDevice>>) {
        self.input_devices.push(dev);
    }

    /// Регистрирует графический адаптер
    pub fn register_display_device(&mut self, dev: Arc<Mutex<dyn DisplayDevice>>) {
        self.display_devices.push(dev);
    }

    /// Регистрирует звуковое устройство
    pub fn register_audio_device(&mut self, dev: Arc<Mutex<dyn AudioDevice>>) {
        self.audio_devices.push(dev);
    }

    /// Возвращает блочное устройство по системному имени
    pub fn get_block_device(&self, name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
        self.block_devices.get(name).cloned()
    }

    /// Возвращает сетевое устройство по его индексу
    pub fn get_net_device(&self, index: usize) -> Option<Arc<Mutex<dyn NetDevice>>> {
        self.net_devices.get(index).cloned()
    }

    /// Возвращает устройство ввода по его индексу
    pub fn get_input_device(&self, index: usize) -> Option<Arc<Mutex<dyn InputDevice>>> {
        self.input_devices.get(index).cloned()
    }

    /// Возвращает графическое устройство по его индексу
    pub fn get_display_device(&self, index: usize) -> Option<Arc<Mutex<dyn DisplayDevice>>> {
        self.display_devices.get(index).cloned()
    }

    /// Возвращает звуковое устройство по его индексу
    pub fn get_audio_device(&self, index: usize) -> Option<Arc<Mutex<dyn AudioDevice>>> {
        self.audio_devices.get(index).cloned()
    }

    /// Список всех обнаруженных устройств на шине PCI
    pub fn pci_devices(&self) -> &[pci::PciDevice] {
        &self.pci_devices
    }

    /// Имена всех зарегистрированных блочных устройств
    pub fn block_device_names(&self) -> Vec<String> {
        self.block_devices.keys().cloned().collect()
    }

    /// Количество активных сетевых адаптеров
    pub fn net_device_count(&self) -> usize {
        self.net_devices.len()
    }

    /// Количество активных устройств ввода
    pub fn input_device_count(&self) -> usize {
        self.input_devices.len()
    }

    /// Количество активных видеоадаптеров
    pub fn display_device_count(&self) -> usize {
        self.display_devices.len()
    }

    /// Количество активных звуковых контроллеров
    pub fn audio_device_count(&self) -> usize {
        self.audio_devices.len()
    }

    /// Сканирует шину PCI, инициализирует контроллеры и интеграции сред виртуализации.
    pub fn init_hardware(&mut self) {
        super::acpi::init();
        super::tpm::init();
        if let Some(mouse) = super::input::ps2_mouse::init() {
            self.register_input_device(mouse);
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

        kboot!(
            "pci0",
            "{}",
            t("<шина PCI>", "<PCI bus>")
        );
        let mut pci_list = pci::scan::scan_all_busses();
        let mut block_count = 0usize;

        for dev in &mut pci_list {
            if dev.vendor_id == virtio::VIRTIO_PCI_VENDOR_ID {
                self.probe_virtio(dev, &mut block_count);
            } else if dev.class_code == 0x01 && dev.subclass == 0x01 {
                self.probe_ide(dev);
            } else if super::ahci::probe_pci(dev, self, &mut block_count) {
                // Контроллер SATA / AHCI обработал устройство
            } else if super::nvme::probe_pci(dev, self) {
                // Высокоскоростной SSD накопитель NVMe (PCI Express)
            } else if virt::vbox::probe_pci(dev, self) {
                dev.driver_name = Some(if dev.device_id == 0xBEEF { "vboxvideo" } else { "vboxguest" });
            } else if virt::vmware::probe_pci(dev, self) {
                dev.driver_name = Some(match dev.device_id {
                    0x0405 => "vmw_svga",
                    0x07B0 => "vmxnet3",
                    0x07C0 => "pvscsi",
                    _ => "vmware",
                });
            } else if virt::hyperv::probe_pci(dev, self) {
                dev.driver_name = Some("hyperv_fb");
            } else if virt::qemu::probe_pci(dev, self) {
                dev.driver_name = Some("bochs-vga");
            } else if virt::xen::platform::probe_pci(dev) {
                dev.driver_name = Some("xen-platform");
            } else if super::net::probe_pci(dev, self) {
                // Сетевые контроллеры Realtek RTL8139 / RTL8168 / RTL8125
            } else if super::usb::probe_pci(dev, self, &mut block_count) {
                // Хост-контроллеры шины USB (xHCI, EHCI, OHCI, UHCI)
            } else if dev.class_code == 0x08 && dev.subclass == 0x05 {
                if super::sdhci::probe(dev, self).is_ok() {
                    dev.driver_name = Some("sdhci");
                }
            } else if super::audio::probe_pci(dev, self) {
                // Звуковые контроллеры Intel HDA и AC'97
            } else if super::i2c::probe_pci(dev, self) {
                // Контроллеры шины I2C (Synopsys DesignWare / Intel LPSS) и тачпады I2C-HID
            } else if let Some(nic) = super::e1000::probe_pci(dev) {
                dev.driver_name = Some("e1000");
                let mac = nic.mac_address();
                kboot!(
                    "e1000",
                    "{} eth{} (Intel PRO/1000, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    t("зарегистрирован интерфейс", "registered interface"),
                    self.net_devices.len(), mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                self.register_net_device(Arc::new(Mutex::new(nic)));
            }
            log_pci_device(dev);
        }

        self.pci_devices = pci_list;
        self.init_hypervisor_integrations(hv);
        super::audio::init_legacy(self);

        kboot!(
            "pci0",
            "{}: {} ({}: {}, {}: {}, {}: {}, {}: {})",
            t("обнаружено устройств", "devices found"),
            self.pci_devices.len(),
            t("дисков", "disks"),
            self.block_devices.len(),
            t("сетей", "net"),
            self.net_devices.len(),
            t("дисплеев", "display"),
            self.display_devices.len(),
            t("ввода", "input"),
            self.input_devices.len()
        );
    }

    /// Инициализация обнаруженного контроллера IDE/ATA
    fn probe_ide(&mut self, dev: &mut pci::PciDevice) {
        dev.driver_name = Some("ata_piix");
        dev.enable_io_space();
        dev.enable_bus_mastering();

        // Проверяем 4 стандартных IDE диска (Primary/Secondary Master/Slave)
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
                self.register_block_device(String::from(name), Arc::new(Mutex::new(ata)));
            }
        }
    }

    /// Инициализация обнаруженных устройств VirtIO.
    fn probe_virtio(&mut self, dev: &mut pci::PciDevice, block_count: &mut usize) {
        let vtype = virtio::VirtioDeviceType::from_pci(dev.device_id, dev.subsystem_id);
        dev.driver_name = Some(match vtype {
            virtio::VirtioDeviceType::Block => "virtio_blk",
            virtio::VirtioDeviceType::Net => "virtio_net",
            virtio::VirtioDeviceType::Gpu => "virtio_gpu",
            virtio::VirtioDeviceType::Input => "virtio_input",
            virtio::VirtioDeviceType::Scsi => "virtio_scsi",
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
                    self.register_block_device(name, Arc::new(Mutex::new(blk)));
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
                    self.register_block_device(name, Arc::new(Mutex::new(scsi)));
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
                        self.net_devices.len(),
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    self.register_net_device(Arc::new(Mutex::new(net)));
                }
            }
            virtio::VirtioDeviceType::Input => {
                if let Ok(input) = virtio::input::VirtioInput::new(transport) {
                    kboot!(
                        "virtio-input",
                        "{}",
                        t("устройство ввода зарегистрировано", "input device registered")
                    );
                    self.register_input_device(Arc::new(Mutex::new(input)));
                }
            }
            virtio::VirtioDeviceType::Gpu => {
                if let Ok(gpu) = virtio::gpu::VirtioGpu::new(transport, 1024, 768) {
                    kboot!(
                        "virtio-gpu",
                        "{} (1024x768)",
                        t("графический 2D-адаптер инициализирован", "2D graphics adapter initialized")
                    );
                    self.register_display_device(Arc::new(Mutex::new(gpu)));
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
                let _ = virtio::console::VirtioConsole::new(transport);
            }
            virtio::VirtioDeviceType::Balloon => {
                let _ = virtio::balloon::VirtioBalloon::new(transport);
            }
            _ => {}
        }
    }

    /// Инициализация специфичных для платформы синтетических драйверов и сервисов.
    fn init_hypervisor_integrations(&mut self, hv: HypervisorType) {
        match hv {
            HypervisorType::VirtualBox => {
                virt::vbox::init_integrations(self);
            }
            HypervisorType::Kvm | HypervisorType::Qemu => {
                virt::qemu::init_integrations(self);
            }
            HypervisorType::VMware => {
                virt::vmware::init_integrations(self);
            }
            HypervisorType::HyperV => {
                virt::hyperv::init_synthetic_drivers(self);
            }
            HypervisorType::Xen => {
                virt::xen::init_xen_drivers(self);
            }
            _ => {}
        }
    }
}

pub static DEVICE_MANAGER: RwLock<Option<DeviceManager>> = RwLock::new(None);

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

/// Инициализирует глобальный менеджер устройств ядра Wunix
pub fn init() {
    let mut dm = DeviceManager::new();
    dm.init_hardware();
    *DEVICE_MANAGER.write() = Some(dm);
}

/// Получает блочное устройство по системному имени (например, "vda")
pub fn get_block_device(name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref()?.get_block_device(name)
}

/// Получает сетевое устройство по его номеру
pub fn get_net_device(index: usize) -> Option<Arc<Mutex<dyn NetDevice>>> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref()?.get_net_device(index)
}

/// Получает устройство ввода по его номеру
pub fn get_input_device(index: usize) -> Option<Arc<Mutex<dyn InputDevice>>> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref()?.get_input_device(index)
}

/// Получает графическое устройство по его номеру
pub fn get_display_device(index: usize) -> Option<Arc<Mutex<dyn DisplayDevice>>> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref()?.get_display_device(index)
}

/// Получает звуковое устройство по его номеру
pub fn get_audio_device(index: usize) -> Option<Arc<Mutex<dyn AudioDevice>>> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref()?.get_audio_device(index)
}

/// Предоставляет доступ к глобальному DeviceManager для чтения информации
pub fn with_device_manager<R>(f: impl FnOnce(&DeviceManager) -> R) -> Option<R> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref().map(f)
}
