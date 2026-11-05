pub mod classes;
pub mod ehci;
pub mod uhci;
pub mod xhci;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Проверяет, является ли PCI устройство хост-контроллером USB (xHCI, EHCI, UHCI)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if xhci::probe_pci(dev, dm) {
        return true;
    }
    if ehci::probe_pci(dev, dm) {
        return true;
    }
    if uhci::probe_pci(dev, dm) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_usb_struct_sizes() {
        assert_eq!(core::mem::size_of::<xhci::trb::Trb>(), 16);
        assert_eq!(core::mem::size_of::<classes::storage::UsbCbw>(), 31);
        assert_eq!(core::mem::size_of::<classes::storage::UsbCsw>(), 13);
        assert_eq!(core::mem::size_of::<classes::hid::UsbKeyboardReport>(), 8);
        assert_eq!(core::mem::size_of::<classes::hid::UsbMouseReport>(), 3);
    }
}
