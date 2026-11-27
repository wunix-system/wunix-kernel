use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::drivers::pci;
use crate::drivers::traits::{AudioDevice, BlockDevice, DisplayDevice, InputDevice, NetDevice};
use crate::drivers::virtio::balloon::VirtioBalloon;
use crate::drivers::virtio::console::VirtioConsole;

/// Реестр устройств ядра Wunix (DeviceManager)
pub struct DeviceManager {
    pub(crate) pci_devices: Vec<pci::PciDevice>,
    pub(crate) block_devices: BTreeMap<String, Arc<Mutex<dyn BlockDevice>>>,
    pub(crate) net_devices: Vec<Arc<Mutex<dyn NetDevice>>>,
    pub(crate) input_devices: Vec<Arc<Mutex<dyn InputDevice>>>,
    pub(crate) display_devices: Vec<Arc<Mutex<dyn DisplayDevice>>>,
    pub(crate) audio_devices: Vec<Arc<Mutex<dyn AudioDevice>>>,
    pub(crate) console_devices: Vec<Arc<Mutex<VirtioConsole>>>,
    pub(crate) balloon_devices: Vec<Arc<Mutex<VirtioBalloon>>>,
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
            console_devices: Vec::new(),
            balloon_devices: Vec::new(),
        }
    }

    pub fn register_block_device(&mut self, name: String, dev: Arc<Mutex<dyn BlockDevice>>) {
        self.block_devices.insert(name, dev);
    }

    pub fn register_net_device(&mut self, dev: Arc<Mutex<dyn NetDevice>>) {
        self.net_devices.push(dev);
    }

    pub fn register_input_device(&mut self, dev: Arc<Mutex<dyn InputDevice>>) {
        self.input_devices.push(dev);
    }

    pub fn register_display_device(&mut self, dev: Arc<Mutex<dyn DisplayDevice>>) {
        self.display_devices.push(dev);
    }

    pub fn register_audio_device(&mut self, dev: Arc<Mutex<dyn AudioDevice>>) {
        self.audio_devices.push(dev);
    }

    pub fn register_console_device(&mut self, dev: Arc<Mutex<VirtioConsole>>) {
        self.console_devices.push(dev);
    }

    pub fn register_balloon_device(&mut self, dev: Arc<Mutex<VirtioBalloon>>) {
        self.balloon_devices.push(dev);
    }

    pub fn get_block_device(&self, name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
        self.block_devices.get(name).cloned()
    }

    pub fn get_net_device(&self, index: usize) -> Option<Arc<Mutex<dyn NetDevice>>> {
        self.net_devices.get(index).cloned()
    }

    pub fn get_input_device(&self, index: usize) -> Option<Arc<Mutex<dyn InputDevice>>> {
        self.input_devices.get(index).cloned()
    }

    pub fn get_display_device(&self, index: usize) -> Option<Arc<Mutex<dyn DisplayDevice>>> {
        self.display_devices.get(index).cloned()
    }

    pub fn get_audio_device(&self, index: usize) -> Option<Arc<Mutex<dyn AudioDevice>>> {
        self.audio_devices.get(index).cloned()
    }

    pub fn get_console_device(&self, index: usize) -> Option<Arc<Mutex<VirtioConsole>>> {
        self.console_devices.get(index).cloned()
    }

    pub fn get_balloon_device(&self, index: usize) -> Option<Arc<Mutex<VirtioBalloon>>> {
        self.balloon_devices.get(index).cloned()
    }

    pub fn pci_devices(&self) -> &[pci::PciDevice] {
        &self.pci_devices
    }

    pub fn block_device_names(&self) -> Vec<String> {
        self.block_devices.keys().cloned().collect()
    }

    pub fn net_device_count(&self) -> usize {
        self.net_devices.len()
    }

    pub fn input_device_count(&self) -> usize {
        self.input_devices.len()
    }

    pub fn display_device_count(&self) -> usize {
        self.display_devices.len()
    }

    pub fn audio_device_count(&self) -> usize {
        self.audio_devices.len()
    }

    pub fn console_device_count(&self) -> usize {
        self.console_devices.len()
    }

    pub fn init_hardware(&mut self) {
        super::probe::init_hardware(self);
    }
}

pub static DEVICE_MANAGER: RwLock<Option<DeviceManager>> = RwLock::new(None);

pub fn init() {
    let mut dm = DeviceManager::new();
    dm.init_hardware();
    *DEVICE_MANAGER.write() = Some(dm);
}

pub fn get_block_device(name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
    DEVICE_MANAGER.read().as_ref()?.get_block_device(name)
}

pub fn get_net_device(index: usize) -> Option<Arc<Mutex<dyn NetDevice>>> {
    DEVICE_MANAGER.read().as_ref()?.get_net_device(index)
}

pub fn get_input_device(index: usize) -> Option<Arc<Mutex<dyn InputDevice>>> {
    DEVICE_MANAGER.read().as_ref()?.get_input_device(index)
}

pub fn get_display_device(index: usize) -> Option<Arc<Mutex<dyn DisplayDevice>>> {
    DEVICE_MANAGER.read().as_ref()?.get_display_device(index)
}

pub fn get_audio_device(index: usize) -> Option<Arc<Mutex<dyn AudioDevice>>> {
    DEVICE_MANAGER.read().as_ref()?.get_audio_device(index)
}

pub fn get_console_device(index: usize) -> Option<Arc<Mutex<VirtioConsole>>> {
    DEVICE_MANAGER.read().as_ref()?.get_console_device(index)
}

pub fn with_device_manager<R>(f: impl FnOnce(&DeviceManager) -> R) -> Option<R> {
    let guard = DEVICE_MANAGER.read();
    guard.as_ref().map(f)
}
