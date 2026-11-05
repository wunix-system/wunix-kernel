use x86_64::instructions::port::Port;

use crate::drivers::pci::PciDevice;
use crate::serial_println;

pub const XEN_PCI_VENDOR_ID: u16 = 0x5853;
pub const XEN_PLATFORM_DEVICE_ID: u16 = 0x0001;

// Протокол отключения эмулируемых устройств (Xen Unplug Protocol)
// Запись в порт 0x10 магического числа 0x49D2 и битовой маски отключает эмулированные BIOS IDE и RTL8139
const XEN_IOPORT_UNPLUG: u16 = 0x10;
const XEN_UNPLUG_MAGIC: u16 = 0x49D2;
const XEN_UNPLUG_ALL_IDE: u16 = 0x01;
const XEN_UNPLUG_ALL_NICS: u16 = 0x02;

/// Драйвер синтетического интерфейса Xen Platform Device (PCI 0x5853:0x0001).
/// В режиме HVM (Hardware-assisted Virtual Machine) уведомляет гипервизор об активации
/// нативных паравиртуальных драйверов ядра Wunix (xen-blkfront, xen-netfront).
pub struct XenPlatformDevice {
    io_port: u16,
}

impl XenPlatformDevice {
    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_io_space();
        let io_bar = pci.first_io_bar().ok_or("xen-platform: I/O BAR not found")?;

        let mut dev = Self {
            io_port: io_bar.address as u16,
        };

        dev.unplug_emulated_devices();
        Ok(dev)
    }

    /// Выполняет отключение эмулированных IDE дисков и сетевых карт в пользу паравиртуальных очередей
    fn unplug_emulated_devices(&mut self) {
        unsafe {
            // Записываем магическую последовательность отключения
            let mut port = Port::<u16>::new(self.io_port + XEN_IOPORT_UNPLUG);
            let magic = XEN_UNPLUG_MAGIC;
            let mask = XEN_UNPLUG_ALL_IDE | XEN_UNPLUG_ALL_NICS;
            port.write(magic);
            port.write(mask);
        }
        serial_println!("xen-platform: unplugged emulated IDE and NICs, activated paravirtual drivers");
    }
}

pub fn probe_pci(dev: &PciDevice) -> bool {
    if dev.vendor_id == XEN_PCI_VENDOR_ID && dev.device_id == XEN_PLATFORM_DEVICE_ID {
        serial_println!("xen: initializing Xen Platform PCI Device (0x5853:0x0001)...");
        match XenPlatformDevice::new(dev) {
            Ok(_) => {
                serial_println!("xen: platform device configured successfully");
            }
            Err(e) => {
                serial_println!("xen: platform device init failed: {}", e);
            }
        }
        return true;
    }
    false
}
