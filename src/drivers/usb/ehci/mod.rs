pub mod regs;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use regs::*;

/// Драйвер контроллера USB 2.0 EHCI (Enhanced Host Controller Interface)
pub struct EhciController {
    _mmio_base: *mut u8,
    op_base: *mut u8,
    port_count: usize,
}

unsafe impl Send for EhciController {}
unsafe impl Sync for EhciController {}

impl EhciController {
    fn read_op32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.op_base.add(offset) as *const u32) }
    }

    fn write_op32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.op_base.add(offset) as *mut u32, val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("ehci: BAR0 not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "ehci: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let caplength = unsafe { core::ptr::read_volatile(mmio_base.add(CAP_CAPLENGTH)) } as usize;
        let op_base = unsafe { mmio_base.add(caplength) };

        let hcsparams = unsafe {
            core::ptr::read_volatile(mmio_base.add(CAP_HCSPARAMS) as *const u32)
        };
        let port_count = (hcsparams & 0x0F) as usize;

        let mut controller = Self {
            _mmio_base: mmio_base,
            op_base,
            port_count,
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        // Останавливаем контроллер
        let cmd = self.read_op32(OP_USBCMD);
        if (cmd & USBCMD_RS) != 0 {
            self.write_op32(OP_USBCMD, cmd & !USBCMD_RS);
            for _ in 0..50_000 {
                if (self.read_op32(OP_USBSTS) & USBSTS_HCHALTED) != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Программный сброс контроллера
        self.write_op32(OP_USBCMD, USBCMD_HCRESET);
        for _ in 0..50_000 {
            if (self.read_op32(OP_USBCMD) & USBCMD_HCRESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Направляем управление портами на драйвер EHCI (CONFIGFLAG = 1)
        self.write_op32(OP_CONFIGFLAG, 1);

        // Подаем питание на все порты
        for port in 0..self.port_count {
            let reg = OP_PORTSC_BASE + (port * 4);
            let val = self.read_op32(reg);
            self.write_op32(reg, val | PORTSC_POWER);
        }

        // Запуск планировщика EHCI
        self.write_op32(OP_USBCMD, USBCMD_RS);

        self.scan_ports();
        Ok(())
    }

    fn scan_ports(&self) {
        for port in 0..self.port_count {
            let reg = OP_PORTSC_BASE + (port * 4);
            let val = self.read_op32(reg);
            if (val & PORTSC_CONNECT) != 0 {
                serial_println!("ehci: port {} device connected (High-Speed USB 2.0)", port + 1);
            }
        }
    }
}

/// Поиск и регистрация контроллеров USB 2.0 EHCI (Class 0x0C, Subclass 0x03, ProgIF 0x20)
pub fn probe_pci(dev: &mut PciDevice, _dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x20 {
        match EhciController::new(dev) {
            Ok(ehci) => {
                dev.driver_name = Some("ehci");
                serial_println!(
                    "ehci: registered USB 2.0 Host Controller ({:04x}:{:04x}, {} ports)",
                    dev.vendor_id, dev.device_id, ehci.port_count
                );
                return true;
            }
            Err(e) => {
                serial_println!("ehci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
