pub mod ac97;
pub mod hda;
pub mod sb16;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Опрос PCI аудиоконтроллеров (Intel HDA, AC'97)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if hda::probe_pci(dev, dm) {
        return true;
    }
    if ac97::probe_pci(dev, dm) {
        return true;
    }
    false
}

/// Инициализация не-PCI винтажного звукового оборудования (Sound Blaster 16)
pub fn init_legacy(dm: &mut DeviceManager) {
    if let Some(sb16) = sb16::init() {
        dm.register_audio_device(Arc::new(Mutex::new(sb16)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_audio_struct_sizes() {
        assert_eq!(core::mem::size_of::<hda::bdl::BdlEntry>(), 16);
        assert_eq!(core::mem::size_of::<ac97::bd::Ac97BufferDesc>(), 8);
    }
}
