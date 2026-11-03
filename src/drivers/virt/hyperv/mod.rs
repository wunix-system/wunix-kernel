pub mod netvsc;
pub mod services;
pub mod storvsc;
pub mod synthvid;
pub mod vmbus;

use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::NetDevice;
use crate::serial_println;

pub use netvsc::NetVscDevice;
pub use services::{HyperVHeartbeatService, HyperVKvpService, HyperVShutdownService};
pub use storvsc::StorVscDevice;
pub use synthvid::{HyperVSynthVidDevice, HYPERV_SYNTHVID_DEVICE_ID, MICROSOFT_PCI_VENDOR_ID};
pub use vmbus::{VmbusChannel, VmbusGuid};

pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id != MICROSOFT_PCI_VENDOR_ID {
        return false;
    }

    match dev.device_id {
        HYPERV_SYNTHVID_DEVICE_ID => {
            serial_println!("hyperv: initializing Microsoft Hyper-V Synthetic Video...");
            match HyperVSynthVidDevice::new(dev, 1024, 768) {
                Ok(disp) => {
                    dm.register_display_device(Arc::new(Mutex::new(disp)));
                    serial_println!("hyperv: registered synthetic video display 1024x768x32bpp");
                }
                Err(e) => {
                    serial_println!("hyperv: synthetic video init failed: {}", e);
                }
            }
            true
        }
        _ => false,
    }
}

pub fn init_synthetic_drivers(dm: &mut DeviceManager) {
    serial_println!("hyperv: initializing VMBus synthetic device clients...");

    match NetVscDevice::new(1) {
        Ok(net) => {
            let mac = net.mac_address();
            serial_println!(
                "hyperv: registered NetVSC NIC (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            dm.register_net_device(Arc::new(Mutex::new(net)));
        }
        Err(e) => {
            serial_println!("hyperv: NetVSC init failed: {}", e);
        }
    }

    match StorVscDevice::new(2) {
        Ok(stor) => {
            let count = dm.block_device_names().len();
            let letter = (b'a' + (count as u8).min(25)) as char;
            let name = format!("sd{}", letter);
            serial_println!("hyperv: registered StorVSC block disk /dev/{}", name);
            dm.register_block_device(name, Arc::new(Mutex::new(stor)));
        }
        Err(e) => {
            serial_println!("hyperv: StorVSC init failed: {}", e);
        }
    }

    if let Ok(mut kvp) = HyperVKvpService::new(3) {
        kvp.report_os_info("Wunix", "0.1.0-release");
        serial_println!("hyperv: reported guest OS info via KVP");
    }
}
