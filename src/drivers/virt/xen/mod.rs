pub mod blkfront;
pub mod grant_table;
pub mod hypercall;
pub mod netfront;
pub mod platform;

use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::NetDevice;
use crate::serial_println;

pub use blkfront::XenBlkFront;
pub use grant_table::{GrantTable, GRANT_TABLE};
pub use hypercall::{hypercall3, xen_console_write, xen_version, XenHypercallPage, XEN_HYPERCALL};
pub use netfront::XenNetFront;

pub fn init_xen_drivers(dm: &mut DeviceManager) {
    serial_println!("xen: initializing Xen paravirtualization stack...");

    match XenHypercallPage::new() {
        Ok(hp) => {
            *XEN_HYPERCALL.lock() = Some(hp);
            if let Some((major, minor)) = xen_version() {
                serial_println!("xen: negotiated hypervisor version {}.{}", major, minor);
            }
        }
        Err(e) => {
            serial_println!("xen: failed to initialize hypercall page: {}", e);
            return;
        }
    }

    match GrantTable::new() {
        Ok(gt) => {
            *GRANT_TABLE.lock() = Some(gt);
            serial_println!("xen: allocated grant table (512 entries)");
        }
        Err(e) => {
            serial_println!("xen: failed to initialize grant table: {}", e);
            return;
        }
    }

    match XenBlkFront::new() {
        Ok(blk) => {
            let count = dm.block_device_names().len();
            let letter = (b'a' + (count as u8).min(25)) as char;
            let name = format!("xvd{}", letter);
            serial_println!("xen: registered block device /dev/{}", name);
            dm.register_block_device(name, Arc::new(Mutex::new(blk)));
        }
        Err(e) => {
            serial_println!("xen: failed to initialize blkfront: {}", e);
        }
    }

    match XenNetFront::new() {
        Ok(net) => {
            let mac = net.mac_address();
            serial_println!(
                "xen: registered netfront NIC (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            dm.register_net_device(Arc::new(Mutex::new(net)));
        }
        Err(e) => {
            serial_println!("xen: failed to initialize netfront: {}", e);
        }
    }
}
