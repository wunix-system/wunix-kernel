pub mod bochs_vga;
pub mod fw_cfg;
pub mod pvclock;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;

pub use bochs_vga::{BochsVgaDevice, BOCHS_PCI_VENDOR_ID, BOCHS_VGA_DEVICE_ID};
pub use fw_cfg::QemuFwCfg;
pub use pvclock::KvmPvClock;

pub static KVM_PVCLOCK: Mutex<Option<KvmPvClock>> = Mutex::new(None);

/// Проверяет принадлежность PCI устройства адаптерам QEMU/Bochs (например, standard VGA).
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id == BOCHS_PCI_VENDOR_ID && dev.device_id == BOCHS_VGA_DEVICE_ID {
        if let Ok(disp) = BochsVgaDevice::new(dev, 1024, 768) {
            dm.register_display_device(Arc::new(Mutex::new(disp)));
        }
        return true;
    }

    false
}

/// Инициализирует синтетические сервисы платформы KVM / Proxmox VE / QEMU (fw_cfg, pvclock).
pub fn init_integrations(_dm: &mut DeviceManager) {
    use crate::i18n::t;

    let mut fw = QemuFwCfg::new();
    if fw.is_present() {
        crate::kboot!(
            "fwcfg0",
            "{}",
            t(
                "интерфейс конфигурации QEMU (порт 0x510)",
                "QEMU firmware config interface (0x510)"
            )
        );
    }

    if let Ok(pv) = KvmPvClock::new() {
        *KVM_PVCLOCK.lock() = Some(pv);
        crate::kboot!(
            "pvclock0",
            "{}",
            t(
                "паравиртуальный таймер KVM активен",
                "KVM paravirtual clock active"
            )
        );
    }
}
