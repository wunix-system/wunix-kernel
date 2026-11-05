pub mod regs;

use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::serial_println;

use regs::*;

/// Драйвер контроллера USB 1.1 UHCI (Universal Host Controller Interface)
pub struct UhciController {
    io_base: u16,
}

unsafe impl Send for UhciController {}
unsafe impl Sync for UhciController {}

impl UhciController {
    fn in16(&self, reg: u16) -> u16 {
        let mut port = PortReadOnly::<u16>::new(self.io_base + reg);
        unsafe { port.read() }
    }

    fn out16(&self, reg: u16, val: u16) {
        let mut port = PortWriteOnly::<u16>::new(self.io_base + reg);
        unsafe { port.write(val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_io_space();
        pci.enable_bus_mastering();

        let bar4 = pci.bar(4).ok_or("uhci: BAR4 I/O not found")?;
        let io_base = bar4.address as u16;

        let mut controller = Self { io_base };
        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        // Программный сброс контроллера UHCI
        self.out16(REG_USBCMD, USBCMD_HCRESET);
        for _ in 0..10_000 {
            if (self.in16(REG_USBCMD) & USBCMD_HCRESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Проверяем статус портов 1 и 2
        let p1 = self.in16(REG_PORTSC1);
        if (p1 & PORTSC_CCS) != 0 {
            serial_println!("uhci: port 1 device detected (USB 1.1 Full/Low Speed)");
        }
        let p2 = self.in16(REG_PORTSC2);
        if (p2 & PORTSC_CCS) != 0 {
            serial_println!("uhci: port 2 device detected (USB 1.1 Full/Low Speed)");
        }

        Ok(())
    }
}

/// Поиск и регистрация контроллеров USB 1.1 UHCI (Class 0x0C, Subclass 0x03, ProgIF 0x00)
pub fn probe_pci(dev: &PciDevice, _dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x00 {
        match UhciController::new(dev) {
            Ok(_) => {
                serial_println!(
                    "uhci: registered USB 1.1 Host Controller ({:04x}:{:04x})",
                    dev.vendor_id, dev.device_id
                );
                return true;
            }
            Err(e) => {
                serial_println!("uhci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
