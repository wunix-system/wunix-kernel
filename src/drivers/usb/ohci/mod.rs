pub mod regs;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use regs::*;

/// Область коммуникаций хост-контроллера OHCI (HCCA, 256 байт)
#[repr(C, align(256))]
#[derive(Clone, Copy)]
pub struct OhciHcca {
    pub hcca_interrupt_table: [u32; 32],
    pub hcca_frame_number: u16,
    pub hcca_pad1: u16,
    pub hcca_done_head: u32,
    pub reserved: [u8; 116],
}

impl Default for OhciHcca {
    fn default() -> Self {
        Self {
            hcca_interrupt_table: [0; 32],
            hcca_frame_number: 0,
            hcca_pad1: 0,
            hcca_done_head: 0,
            reserved: [0; 116],
        }
    }
}

/// Драйвер контроллера USB 1.1 OHCI (Open Host Controller Interface)
pub struct OhciController {
    mmio_base: *mut u8,
    port_count: usize,
    hcca: Dma<OhciHcca>,
}

unsafe impl Send for OhciController {}
unsafe impl Sync for OhciController {}

impl OhciController {
    fn read_reg32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32) }
    }

    fn write_reg32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("ohci: BAR0 MMIO not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "ohci: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let desc_a = unsafe { core::ptr::read_volatile(mmio_base.add(REG_RH_DESCRIPTOR_A) as *const u32) };
        let port_count = (desc_a & 0xFF) as usize;

        let hcca = Dma::zeroed()?;

        let mut controller = Self {
            mmio_base,
            port_count,
            hcca,
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        // Программный сброс контроллера OHCI (HCR)
        self.write_reg32(REG_COMMAND_STATUS, CMD_HCR);
        for _ in 0..10_000 {
            if (self.read_reg32(REG_COMMAND_STATUS) & CMD_HCR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Передаем физический адрес HCCA
        self.write_reg32(REG_HCCA, self.hcca.physical() as u32);

        // Настройка интервала кадров (12 000 бит при 12 Мбит/с Full Speed)
        let fminterval = 0x2EDF | (((0x2EDF - 210) * 6 / 7) << 16);
        self.write_reg32(REG_FM_INTERVAL, fminterval);
        self.write_reg32(REG_PERIODIC_START, (0x2EDF * 9) / 10);

        // Перевод контроллера в рабочее состояние (Operational)
        self.write_reg32(
            REG_CONTROL,
            CONTROL_HCFS_OPERATIONAL | CONTROL_PLE | CONTROL_CLE | CONTROL_BLE,
        );

        // Подача питания на порты корневого концентратора
        for p in 0..self.port_count {
            let offset = REG_RH_PORT_STATUS_BASE + (p * 4);
            self.write_reg32(offset, RH_PORT_PPS);
        }

        self.scan_ports();
        Ok(())
    }

    fn scan_ports(&self) {
        for p in 0..self.port_count {
            let offset = REG_RH_PORT_STATUS_BASE + (p * 4);
            let val = self.read_reg32(offset);
            if (val & RH_PORT_CCS) != 0 {
                serial_println!("ohci: port {} device connected (USB 1.1)", p + 1);
            }
        }
    }
}

pub fn probe_pci(dev: &mut PciDevice, _dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x10 {
        match OhciController::new(dev) {
            Ok(ohci) => {
                dev.driver_name = Some("ohci");
                serial_println!(
                    "ohci: registered USB 1.1 Host Controller ({:04x}:{:04x}, {} ports)",
                    dev.vendor_id, dev.device_id, ohci.port_count
                );
                return true;
            }
            Err(e) => {
                serial_println!("ohci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
