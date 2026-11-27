pub mod designware;

use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

pub use designware::DesignwareI2c;

/// Проверяет PCI устройство на принадлежность к контроллерам Intel LPSS / Synopsys DesignWare I2C
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    let is_intel_i2c = dev.vendor_id == 0x8086 && match dev.device_id {
        0x9D60..=0x9D65 | 0xA160..=0xA162 | 0x02E8..=0x02EB | 0x06E8..=0x06EB
        | 0xA0E8..=0xA0EB | 0x51E8..=0x51EB | 0x7AE8..=0x7AEB
        | 0x22C1..=0x22C7 | 0x0F41..=0x0F47 => true,
        _ => false,
    };

    let is_synopsys = dev.class_code == 0x0C && dev.subclass == 0x80;

    if !is_intel_i2c && !is_synopsys {
        return false;
    }

    dev.enable_memory_space();
    dev.enable_bus_mastering();

    let bar0 = match dev.bar(0) {
        Some(b) => b,
        None => return false,
    };

    let mmio = match map_kernel_mmio(bar0.address, bar0.size as usize) {
        Ok(v) => v.as_u64(),
        Err(_) => return false,
    };

    let mut i2c = DesignwareI2c::new(mmio);
    if i2c.init().is_err() {
        return false;
    }

    dev.driver_name = Some("i2c-designware");
    serial_println!(
        "i2c: registered DesignWare I2C controller at {:?} ({:#06x}:{:#06x})",
        dev.address, dev.vendor_id, dev.device_id
    );

    // Опрашиваем стандартные I2C адреса тачпадов ноутбуков (Synaptics, Elan, ALPS)
    let (screen_w, screen_h) = crate::drivers::display::screen_dimensions().unwrap_or((1024, 768));
    let probe_addrs = [0x15, 0x20, 0x2C, 0x30, 0x40, 0x4A, 0x4C];
    for &addr in &probe_addrs {
        if let Ok(touchpad) =
            crate::drivers::input::i2c_hid::I2cTouchpad::probe(&mut i2c, mmio, addr, screen_w, screen_h)
        {
            dm.register_input_device(Arc::new(Mutex::new(touchpad)));
            serial_println!("i2c: attached I2C-HID Touchpad at 7-bit addr {:#04x}", addr);
            break;
        }
    }

    true
}
