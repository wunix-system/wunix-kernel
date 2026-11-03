pub mod balloon;
pub mod blk;
pub mod console;
pub mod gpu;
pub mod input;
pub mod net;
pub mod queue;
pub mod rng;
pub mod scsi;
pub mod transport;

use core::fmt;

// Статусные биты VirtIO (Device Status Field)
pub const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
pub const VIRTIO_STATUS_DEVICE_NEEDS_RESET: u8 = 64;
pub const VIRTIO_STATUS_FAILED: u8 = 128;

// Идентификаторы устройств VirtIO (Device IDs)
pub const VIRTIO_DEV_NET: u16 = 1;
pub const VIRTIO_DEV_BLOCK: u16 = 2;
pub const VIRTIO_DEV_CONSOLE: u16 = 3;
pub const VIRTIO_DEV_ENTROPY: u16 = 4;
pub const VIRTIO_DEV_BALLOON: u16 = 5;
pub const VIRTIO_DEV_SCSI: u16 = 8;
pub const VIRTIO_DEV_GPU: u16 = 16;
pub const VIRTIO_DEV_INPUT: u16 = 18;

// Vendor ID для PCI VirtIO устройств (Red Hat / QEMU)
pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;

// Диапазоны Device ID для шины PCI
pub const VIRTIO_PCI_LEGACY_DEVICE_ID_MIN: u16 = 0x1000;
pub const VIRTIO_PCI_LEGACY_DEVICE_ID_MAX: u16 = 0x103F;
pub const VIRTIO_PCI_MODERN_DEVICE_ID_MIN: u16 = 0x1040;
pub const VIRTIO_PCI_MODERN_DEVICE_ID_MAX: u16 = 0x107F;

/// Тип устройства VirtIO
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioDeviceType {
    Net,
    Block,
    Console,
    Entropy,
    Balloon,
    Scsi,
    Gpu,
    Input,
    Unknown(u16),
}

impl VirtioDeviceType {
    /// Определяет тип VirtIO устройства по кодам PCI Device ID и Subsystem ID.
    pub fn from_pci(device_id: u16, subsystem_id: u16) -> Self {
        // Для Transitional / Legacy устройств (0x1000..0x103F):
        // тип устройства определяется по Subsystem ID, либо по device_id
        if (VIRTIO_PCI_LEGACY_DEVICE_ID_MIN..=VIRTIO_PCI_LEGACY_DEVICE_ID_MAX).contains(&device_id) {
            let id = if subsystem_id != 0 {
                subsystem_id
            } else {
                match device_id {
                    0x1000 => VIRTIO_DEV_NET,
                    0x1001 => VIRTIO_DEV_BLOCK,
                    0x1002 => VIRTIO_DEV_BALLOON,
                    0x1003 => VIRTIO_DEV_CONSOLE,
                    0x1004 => VIRTIO_DEV_SCSI,
                    0x1005 => VIRTIO_DEV_ENTROPY,
                    other => other - 0x1000,
                }
            };

            return match id {
                VIRTIO_DEV_NET => Self::Net,
                VIRTIO_DEV_BLOCK => Self::Block,
                VIRTIO_DEV_CONSOLE => Self::Console,
                VIRTIO_DEV_ENTROPY => Self::Entropy,
                VIRTIO_DEV_BALLOON => Self::Balloon,
                VIRTIO_DEV_SCSI => Self::Scsi,
                _ => Self::Unknown(id),
            };
        }

        // Для Modern устройств (0x1040..=0x107F):
        // Device ID = 0x1040 + Virtio Device ID
        if (VIRTIO_PCI_MODERN_DEVICE_ID_MIN..=VIRTIO_PCI_MODERN_DEVICE_ID_MAX).contains(&device_id) {
            let virtio_id = device_id - 0x1040;
            return match virtio_id {
                VIRTIO_DEV_NET => Self::Net,
                VIRTIO_DEV_BLOCK => Self::Block,
                VIRTIO_DEV_CONSOLE => Self::Console,
                VIRTIO_DEV_ENTROPY => Self::Entropy,
                VIRTIO_DEV_BALLOON => Self::Balloon,
                VIRTIO_DEV_SCSI => Self::Scsi,
                VIRTIO_DEV_GPU => Self::Gpu,
                VIRTIO_DEV_INPUT => Self::Input,
                _ => Self::Unknown(virtio_id),
            };
        }

        // В QEMU некоторые переходные устройства (например, GPU и Input)
        // используют прямые идентификаторы 0x1050 и 0x1052
        match device_id {
            0x1050 => Self::Gpu,
            0x1052 => Self::Input,
            other => Self::Unknown(other),
        }
    }
}

impl fmt::Display for VirtioDeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Net => write!(f, "VirtIO Network Adapter"),
            Self::Block => write!(f, "VirtIO Block Device"),
            Self::Console => write!(f, "VirtIO Console"),
            Self::Entropy => write!(f, "VirtIO RNG"),
            Self::Balloon => write!(f, "VirtIO Memory Balloon"),
            Self::Scsi => write!(f, "VirtIO SCSI Controller"),
            Self::Gpu => write!(f, "VirtIO GPU Display"),
            Self::Input => write!(f, "VirtIO Input Device"),
            Self::Unknown(id) => write!(f, "VirtIO Unknown (ID: {:#x})", id),
        }
    }
}

/// Ошибки инициализации и работы VirtIO
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioError {
    DeviceNotFound,
    InvalidBar,
    QueueTooSmall,
    QueueFull,
    DeviceError(u8),
    IoError,
    Unsupported,
    AllocationFailed,
}

/// Базовый интерфейс устройства VirtIO
pub trait VirtioDevice: Send + Sync {
    fn name(&self) -> &str;
    fn device_type(&self) -> VirtioDeviceType;
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_virtio_device_type_detection() {
        assert_eq!(
            VirtioDeviceType::from_pci(0x1000, 1),
            VirtioDeviceType::Net
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1001, 2),
            VirtioDeviceType::Block
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1004, 0),
            VirtioDeviceType::Scsi
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1005, 0),
            VirtioDeviceType::Entropy
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1041, 0),
            VirtioDeviceType::Net
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1042, 0),
            VirtioDeviceType::Block
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1048, 0),
            VirtioDeviceType::Scsi
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1050, 0),
            VirtioDeviceType::Gpu
        );
        assert_eq!(
            VirtioDeviceType::from_pci(0x1052, 0),
            VirtioDeviceType::Input
        );
    }
}
