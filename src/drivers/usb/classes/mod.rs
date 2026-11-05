pub mod audio;
pub mod cdc_net;
pub mod hid;
pub mod hub;
pub mod serial;
pub mod storage;

pub use audio::UsbAudioDevice;
pub use cdc_net::CdcNetDevice;
pub use hub::UsbHub;
pub use serial::UsbSerialPort;

pub use hid::{UsbKeyboardReport, UsbMouseDevice, UsbMouseReport};
pub use storage::UsbStorageDevice;
