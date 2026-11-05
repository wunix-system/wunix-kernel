pub mod rtl8125;
pub mod rtl8139;
pub mod rtl8168;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Проверяет, поддерживается ли сетевой адаптер нативными драйверами Wunix
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    if rtl8125::probe_pci(dev, dm) {
        return true;
    }
    if rtl8168::probe_pci(dev, dm) {
        return true;
    }
    if rtl8139::probe_pci(dev, dm) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_rtl8168_desc_size() {
        assert_eq!(core::mem::size_of::<rtl8168::desc::RtlDesc>(), 16);
        assert_eq!(core::mem::size_of::<rtl8125::desc::Rtl8125Desc>(), 16);
    }
}
