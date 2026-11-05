pub mod ahci;
pub mod ata;
pub mod audio;
pub mod display;
pub mod dma;
pub mod e1000;
pub mod font;
pub mod io;
pub mod keyboard;
pub mod manager;
pub mod net;
pub mod nvme;
pub mod pci;
pub mod registry;
pub mod rtc;
pub mod serial;
pub mod traits;
pub mod usb;
pub mod virt;
pub mod virtio;

pub use dma::Dma;
pub use io::{Io, Mmio, Pio};
pub use manager::{
    get_audio_device, get_block_device, get_display_device, get_input_device, get_net_device, init,
    with_device_manager, DeviceManager,
};
pub use registry::{list_drivers, DriverInfo};
pub use traits::{
    AudioDevice, AudioDeviceError, BlockDevice, BlockDeviceError, DisplayDevice,
    DisplayDeviceError, InputDevice, InputEvent, NetDevice, NetDeviceError,
};
pub use virtio::VirtioError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_device_manager_initial_state() {
        let dm = DeviceManager::new();
        assert_eq!(dm.pci_devices().len(), 0);
        assert!(dm.get_block_device("vda").is_none());
        assert!(dm.get_net_device(0).is_none());
        assert!(dm.get_input_device(0).is_none());
        assert!(dm.get_display_device(0).is_none());
    }
}
