pub mod iwlwifi;
pub mod ixgbe;
pub mod rtl8125;
pub mod rtl8139;
pub mod rtl8168;
pub mod tg3;
pub mod wifi;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

/// Проверяет, поддерживается ли сетевой адаптер нативными драйверами Wunix
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    // 1. Intel 10GbE (ixgbe: 82599, X540, X550)
    if dev.vendor_id == 0x8086 {
        match dev.device_id {
            0x10FB | 0x10F7 | 0x10F8 | 0x10F9 | 0x1528 | 0x1563 | 0x10B6 => {
                if ixgbe::probe(dev, dm).is_ok() {
                    dev.driver_name = Some("intel-ixgbe");
                    return true;
                }
            }
            // Intel Wi-Fi 6/6E/7 (iwlwifi: AX200, AX201, AX210, 9260, 8265, 7265)
            0x2723 | 0x06F0 | 0x2725 | 0x272B | 0x2526 | 0x24FD | 0x095A => {
                if iwlwifi::probe(dev, dm).is_ok() {
                    dev.driver_name = Some("intel-iwlwifi");
                    return true;
                }
            }
            _ => {}
        }
    }

    // 2. Broadcom Tigon3 Gigabit Ethernet (tg3)
    if dev.vendor_id == 0x14E4 {
        match dev.device_id {
            0x1600 | 0x1659 | 0x165A | 0x165F | 0x1677 | 0x1680 | 0x1692 | 0x1644..=0x1648 => {
                if tg3::probe(dev, dm).is_ok() {
                    dev.driver_name = Some("broadcom-tg3");
                    return true;
                }
            }
            _ => {}
        }
    }

    // 3. Realtek 2.5GbE, 1GbE, 100M
    if rtl8125::probe_pci(dev, dm) {
        dev.driver_name = Some("rtl8125");
        return true;
    }
    if rtl8168::probe_pci(dev, dm) {
        dev.driver_name = Some("rtl8168");
        return true;
    }
    if rtl8139::probe_pci(dev, dm) {
        dev.driver_name = Some("rtl8139");
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
