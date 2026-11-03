pub mod backdoor;
pub mod pvscsi;
pub mod svga;
pub mod vmmouse;
pub mod vmxnet3;

use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{BlockDevice, NetDevice};
use crate::serial_println;

pub use backdoor::{backdoor_call, get_host_time_seconds, is_vmware_present, BackdoorResult};
pub use pvscsi::PvscsiDevice;
pub use svga::VmwareSvgaDevice;
pub use vmmouse::VmwareMouseDevice;
pub use vmxnet3::Vmxnet3Device;

pub const VMWARE_PCI_VENDOR_ID: u16 = 0x15AD;
pub const VMWARE_SVGA2_DEVICE_ID: u16 = 0x0405;
pub const VMWARE_PVSCSI_DEVICE_ID: u16 = 0x07C0;
pub const VMWARE_VMXNET3_DEVICE_ID: u16 = 0x07B0;

pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id != VMWARE_PCI_VENDOR_ID {
        return false;
    }

    match dev.device_id {
        VMWARE_SVGA2_DEVICE_ID => {
            serial_println!("vmware: initializing VMware SVGA II display controller...");
            match VmwareSvgaDevice::new(dev, 1024, 768) {
                Ok(disp) => {
                    dm.register_display_device(Arc::new(Mutex::new(disp)));
                    serial_println!("vmware: registered SVGA II display 1024x768x32bpp");
                }
                Err(e) => {
                    serial_println!("vmware: SVGA II init failed: {}", e);
                }
            }
            true
        }
        VMWARE_VMXNET3_DEVICE_ID => {
            serial_println!("vmware: initializing VMware vmxnet3 virtual NIC...");
            match Vmxnet3Device::new(dev) {
                Ok(net) => {
                    let mac = net.mac_address();
                    serial_println!(
                        "vmware: registered vmxnet3 (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    dm.register_net_device(Arc::new(Mutex::new(net)));
                }
                Err(e) => {
                    serial_println!("vmware: vmxnet3 init failed: {}", e);
                }
            }
            true
        }
        VMWARE_PVSCSI_DEVICE_ID => {
            serial_println!("vmware: initializing VMware PVSCSI paravirtual storage...");
            match PvscsiDevice::new(dev) {
                Ok(scsi) => {
                    let count = dm.block_device_names().len();
                    let letter = (b'a' + (count as u8).min(25)) as char;
                    let name = format!("sd{}", letter);
                    let cap_mb = (scsi.sector_count() * scsi.sector_size() as u64) / (1024 * 1024);
                    serial_println!(
                        "vmware: registered /dev/{} ({} MB, {} sectors)",
                        name, cap_mb, scsi.sector_count()
                    );
                    dm.register_block_device(name, Arc::new(Mutex::new(scsi)));
                }
                Err(e) => {
                    serial_println!("vmware: PVSCSI init failed: {}", e);
                }
            }
            true
        }
        _ => false,
    }
}

pub fn init_integrations(dm: &mut DeviceManager) {
    if is_vmware_present() {
        serial_println!("vmware: detected active VMware Backdoor I/O channel");
        if let Some(host_time) = get_host_time_seconds() {
            serial_println!("vmware: synchronized host time: {} sec (UTC)", host_time);
        }

        if let Ok(mouse) = VmwareMouseDevice::new(1024, 768) {
            dm.register_input_device(Arc::new(Mutex::new(mouse)));
            serial_println!("vmware: registered VMMOUSE absolute mouse integration");
        }
    }
}
