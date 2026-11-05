pub mod bochs_vga;
pub mod fw_cfg;
pub mod pvclock;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::serial_println;

pub use bochs_vga::{BochsVgaDevice, BOCHS_PCI_VENDOR_ID, BOCHS_VGA_DEVICE_ID};
pub use fw_cfg::QemuFwCfg;
pub use pvclock::KvmPvClock;

pub static KVM_PVCLOCK: Mutex<Option<KvmPvClock>> = Mutex::new(None);

/// Проверяет принадлежность PCI устройства адаптерам QEMU/Bochs (например, standard VGA).
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id == BOCHS_PCI_VENDOR_ID && dev.device_id == BOCHS_VGA_DEVICE_ID {
        serial_println!("qemu: initializing standard Bochs/QEMU VGA adapter...");
        match BochsVgaDevice::new(dev, 1024, 768) {
            Ok(disp) => {
                dm.register_display_device(Arc::new(Mutex::new(disp)));
                serial_println!("qemu: registered Bochs VGA display 1024x768x32bpp");
            }
            Err(e) => {
                serial_println!("qemu: Bochs VGA init failed: {}", e);
            }
        }
        return true;
    }

    false
}

/// Инициализирует синтетические сервисы платформы KVM / Proxmox VE / QEMU (fw_cfg, pvclock).
pub fn init_integrations(_dm: &mut DeviceManager) {
    let mut fw = QemuFwCfg::new();
    if fw.is_present() {
        serial_println!("qemu: detected active QEMU fw_cfg paravirtual interface");
        if let Some(ram_b) = fw.ram_size() {
            serial_println!("qemu: fw_cfg reported guest RAM size: {} MB", ram_b / (1024 * 1024));
        }
        if let Some(cpus) = fw.cpu_count() {
            serial_println!("qemu: fw_cfg reported online vCPUs: {}", cpus);
        }
    }

    match KvmPvClock::new() {
        Ok(pv) => {
            if let Some((sec, nsec)) = pv.read_time() {
                serial_println!("kvm: synchronized with host pvclock: {}.{:09} sec UTC", sec, nsec);
            }
            *KVM_PVCLOCK.lock() = Some(pv);
        }
        Err(_) => {
            // Если гипервизор не предоставляет MSR pvclock (например, старый QEMU TCG), тихо пропускаем
        }
    }
}
