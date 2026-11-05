pub mod hgcm;
pub mod mouse;
pub mod timesync;
pub mod vboxsf;
pub mod video;
pub mod vmmdev;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::serial_println;

pub use mouse::VBoxMouseDevice;
pub use timesync::{report_guest_os, sync_time_with_host};
pub use vboxsf::VBoxSfFileSystem;
pub use video::VBoxVideoDevice;
pub use vmmdev::{VBoxGuestDevice, VBOX_GUEST, VBOX_PCI_VENDOR_ID, VBOX_VIDEO_DEVICE_ID, VBOX_VMMDEV_DEVICE_ID};

/// Проверяет PCI устройство на принадлежность VirtualBox и инициализирует драйвер.
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id != VBOX_PCI_VENDOR_ID {
        return false;
    }

    match dev.device_id {
        VBOX_VMMDEV_DEVICE_ID => {
            serial_println!("vbox: initializing VirtualBox Guest VMMDev integration...");
            match VBoxGuestDevice::new(dev) {
                Ok(guest) => {
                    *VBOX_GUEST.lock() = Some(guest);
                    let _ = report_guest_os();
                    if let Some(host_ms) = sync_time_with_host() {
                        serial_println!("vbox: synchronized with host UTC timestamp: {} ms", host_ms);
                    }
                    let mouse = Arc::new(Mutex::new(VBoxMouseDevice::new(1024, 768)));
                    dm.register_input_device(mouse);
                    serial_println!("vbox: registered VirtualBox mouse integration");
                }
                Err(e) => {
                    serial_println!("vbox: failed to initialize VMMDev: {}", e);
                }
            }
            true
        }
        VBOX_VIDEO_DEVICE_ID => {
            serial_println!("vbox: initializing VirtualBox Graphics Adapter (VBoxSVGA)...");
            match VBoxVideoDevice::new(dev, 1024, 768) {
                Ok(disp) => {
                    dm.register_display_device(Arc::new(Mutex::new(disp)));
                    serial_println!("vbox: registered display 1024x768x32bpp");
                }
                Err(e) => {
                    serial_println!("vbox: failed to initialize video adapter: {}", e);
                }
            }
            true
        }
        _ => false,
    }
}

/// Инициализирует глобальные сервисы интеграции гостевой ОС VirtualBox.
pub fn init_integrations(_dm: &mut DeviceManager) {
    if VBOX_GUEST.lock().is_some() {
        serial_println!("vbox: guest services and integrations active");
        if let Some(host_ms) = sync_time_with_host() {
            serial_println!("vbox: host time sync check: {} ms UTC", host_ms);
        }
    }
}

