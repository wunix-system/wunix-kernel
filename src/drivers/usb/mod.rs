pub mod classes;
pub mod ehci;
pub mod host;
pub mod ohci;
pub mod uhci;
pub mod xhci;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Проверяет, является ли PCI устройство хост-контроллером USB (xHCI, EHCI, OHCI, UHCI)
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if xhci::probe_pci(dev, dm, block_count) {
        return true;
    }
    if ehci::probe_pci(dev, dm, block_count) {
        return true;
    }
    if ohci::probe_pci(dev, dm, block_count) {
        return true;
    }
    if uhci::probe_pci(dev, dm, block_count) {
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

    #[test_case]
    fn test_xhci_dci_and_usb_descriptor_parser() {
        assert_eq!(xhci::device::endpoint_dci(0x81), Some(3));
        assert_eq!(xhci::device::endpoint_dci(0x02), Some(4));
        assert_eq!(xhci::device::endpoint_dci(0x00), None);

        // Синтетический USB Configuration Descriptor: Interface 0x08 (Mass Storage) + EP 0x81 IN + EP 0x02 OUT
        let raw_cfg: [u8; 32] = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50,
            9, 4, 0, 0, 2, 0x08, 0x06, 0x50, 0,
            7, 5, 0x81, 0x02, 0x00, 0x02, 0,
            7, 5, 0x02, 0x02, 0x00, 0x02, 0,
        ];
        let ifaces = host::parse_config_interfaces(&raw_cfg);
        assert_eq!(ifaces.len(), 1);
        assert_eq!(ifaces[0].class, 0x08);
        assert_eq!(ifaces[0].endpoints.len(), 2);
        let (bin, bout, intr) = host::summarize_endpoints(&ifaces[0].endpoints);
        assert_eq!((bin, bout, intr), (0x81, 0x02, 0));
    }
}
