use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::drivers::pci;
use crate::drivers::traits::{BlockDevice, DisplayDevice, InputDevice, NetDevice};
use crate::drivers::virt::{self, HypervisorType};
use crate::drivers::virtio;
use crate::serial_println;

/// Реестр устройств ядра Wunix (DeviceManager)
pub struct DeviceManager {
    pub(crate) pci_devices: Vec<pci::PciDevice>,
    pub(crate) block_devices: BTreeMap<String, Arc<Mutex<dyn BlockDevice>>>,
    pub(crate) net_devices: Vec<Arc<Mutex<dyn NetDevice>>>,
    pub(crate) input_devices: Vec<Arc<Mutex<dyn InputDevice>>>,
    pub(crate) display_devices: Vec<Arc<Mutex<dyn DisplayDevice>>>,
}

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            pci_devices: Vec::new(),
            block_devices: BTreeMap::new(),
            net_devices: Vec::new(),
            input_devices: Vec::new(),
            display_devices: Vec::new(),
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

    /// Сканирует шину PCI, инициализирует контроллеры и интеграции сред виртуализации.
    pub fn init_hardware(&mut self) {
        let hv = virt::init();
        serial_println!("virt: detected hypervisor: {} ({})", hv.name(), hv.display_name(true));

        serial_println!("pci: scanning bus topology...");
        let pci_list = pci::scan::scan_all_busses();
        let mut block_count = 0usize;

        for dev in &pci_list {
            let class_str = pci::PciClass::name(dev.class_code, dev.subclass);
            serial_println!(
                "pci: [{}] {:04x}:{:04x} class: {:02x}:{:02x} ({})",
                dev.address, dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, class_str
            );

            if dev.vendor_id == virtio::VIRTIO_PCI_VENDOR_ID {
                self.probe_virtio(dev, &mut block_count);
            } else if dev.class_code == 0x01 && dev.subclass == 0x01 {
                self.probe_ide(dev);
            } else if super::ahci::probe_pci(dev, self, &mut block_count) {
                // Контроллер SATA / AHCI обработал устройство
            } else if super::nvme::probe_pci(dev, self) {
                // Высокоскоростной SSD накопитель NVMe (PCI Express)
            } else if virt::vbox::probe_pci(dev, self) {
                // VirtualBox драйвер обработал устройство
            } else if virt::vmware::probe_pci(dev, self) {
                // VMware драйвер обработал устройство
            } else if virt::hyperv::probe_pci(dev, self) {
                // Hyper-V синтетическое видео обработано
            } else if virt::qemu::probe_pci(dev, self) {
                // Bochs / QEMU Standard VGA
            } else if virt::xen::platform::probe_pci(dev) {
                // Xen Platform Device (HVM Unplug)
            } else if super::net::probe_pci(dev, self) {
                // Сетевые контроллеры Realtek RTL8139 / RTL8168
            } else if let Some(nic) = super::e1000::probe_pci(dev) {
                let mac = nic.mac_address();
                serial_println!(
                    "e1000: registered eth{} (Intel PRO/1000, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    self.net_devices.len(), mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                self.register_net_device(Arc::new(Mutex::new(nic)));
            }
        }

        self.pci_devices = pci_list;
        self.init_hypervisor_integrations(hv);
    }

    /// Инициализация обнаруженного контроллера IDE/ATA
    fn probe_ide(&mut self, dev: &pci::PciDevice) {
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
                serial_println!(
                    "ata: registered /dev/{} ({} MB, {} sectors)",
                    name,
                    cap_mb,
                    ata.sector_count()
                );
                self.register_block_device(String::from(name), Arc::new(Mutex::new(ata)));
            }
        }
    }

    /// Инициализация обнаруженных устройств VirtIO.
    fn probe_virtio(&mut self, dev: &pci::PciDevice, block_count: &mut usize) {
        let vtype = virtio::VirtioDeviceType::from_pci(dev.device_id, dev.subsystem_id);
        serial_println!("virtio: found {} at {}", vtype, dev.address);

        let transport = match virtio::transport::create_transport_from_pci(dev) {
            Ok(t) => t,
            Err(e) => {
                serial_println!("virtio: transport creation failed for {}: {:?}", dev.address, e);
                return;
            }
        };

        match vtype {
            virtio::VirtioDeviceType::Block => {
                if let Ok(blk) = virtio::blk::VirtioBlock::new(transport) {
                    let letter = (b'a' + (*block_count as u8).min(25)) as char;
                    let name = format!("vd{}", letter);
                    let cap_mb = (blk.sector_count() * 512) / (1024 * 1024);
                    serial_println!("virtio-blk: registered /dev/{} ({} MB)", name, cap_mb);
                    self.register_block_device(name, Arc::new(Mutex::new(blk)));
                    *block_count += 1;
                }
            }
            virtio::VirtioDeviceType::Scsi => {
                match virtio::scsi::VirtioScsi::new(transport) {
                    Ok(scsi) => {
                        let letter = (b'a' + (*block_count as u8).min(25)) as char;
                        let name = format!("sd{}", letter);
                        let cap_mb =
                            (scsi.sector_count() * scsi.sector_size() as u64) / (1024 * 1024);
                        serial_println!("virtio-scsi: registered /dev/{} ({} MB)", name, cap_mb);
                        self.register_block_device(name, Arc::new(Mutex::new(scsi)));
                        *block_count += 1;
                    }
                    Err(e) => {
                        serial_println!("virtio-scsi: init failed: {:?}", e);
                    }
                }
            }
            virtio::VirtioDeviceType::Net => {
                if let Ok(net) = virtio::net::VirtioNet::new(transport) {
                    let m = net.mac_address();
                    serial_println!("virtio-net: registered eth{} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                        self.net_devices.len(), m[0], m[1], m[2], m[3], m[4], m[5]);
                    self.register_net_device(Arc::new(Mutex::new(net)));
                }
            }
            virtio::VirtioDeviceType::Input => {
                if let Ok(input) = virtio::input::VirtioInput::new(transport) {
                    serial_println!("virtio-input: registered input{} ('{}')", self.input_devices.len(), input.name());
                    self.register_input_device(Arc::new(Mutex::new(input)));
                }
            }
            virtio::VirtioDeviceType::Gpu => {
                if let Ok(gpu) = virtio::gpu::VirtioGpu::new(transport, 1024, 768) {
                    let (w, h) = gpu.resolution();
                    serial_println!("virtio-gpu: registered gpu{} ({}x{})", self.display_devices.len(), w, h);
                    self.register_display_device(Arc::new(Mutex::new(gpu)));
                }
            }
            virtio::VirtioDeviceType::Entropy => {
                if let Ok(rng) = virtio::rng::VirtioRng::new(transport) {
                    serial_println!("virtio-rng: registered hardware entropy source");
                    *virtio::rng::VIRTIO_RNG.lock() = Some(rng);
                }
            }
            virtio::VirtioDeviceType::Console => {
                if let Ok(_cons) = virtio::console::VirtioConsole::new(transport) {
                    serial_println!("virtio-console: registered virtual console device /dev/hvc0");
                }
            }
            virtio::VirtioDeviceType::Balloon => {
                if let Ok(_balloon) = virtio::balloon::VirtioBalloon::new(transport) {
                    serial_println!("virtio-balloon: registered dynamic memory ballooning service");
                }
            }
            _ => {
                serial_println!("virtio: device type {:?} not activated", vtype);
            }
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

/// Предоставляет доступ к глобальному DeviceManager для чтения информации
pub fn with_device_manager<R>(f: impl FnOnce(&DeviceManager) -> R) -> Option<R> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref().map(f)
}
