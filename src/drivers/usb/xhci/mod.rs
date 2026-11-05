pub mod regs;
pub mod trb;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::dma::Dma;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use regs::*;
use trb::{Trb, TRB_CYCLE, TRB_TC, TRB_TYPE_LINK};

const CMD_RING_SIZE: usize = 64;

/// Драйвер контроллера USB 3.0/3.1/3.2 xHCI (eXtensible Host Controller Interface)
pub struct XhciController {
    _mmio_base: *mut u8,
    op_base: *mut u8,
    max_ports: usize,
    max_slots: usize,
    cmd_ring: Dma<[Trb; CMD_RING_SIZE]>,
    _cmd_enqueue: usize,
    _cmd_cycle: bool,
    dcbaa: Dma<[u64; 256]>,
}

unsafe impl Send for XhciController {}
unsafe impl Sync for XhciController {}

impl XhciController {
    fn read_op32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.op_base.add(offset) as *const u32) }
    }

    fn write_op32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.op_base.add(offset) as *mut u32, val) }
    }

    fn write_op64(&self, offset: usize, val: u64) {
        unsafe { core::ptr::write_volatile(self.op_base.add(offset) as *mut u64, val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("xhci: BAR0 not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "xhci: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let caplength = unsafe { core::ptr::read_volatile(mmio_base.add(CAP_CAPLENGTH)) } as usize;
        let op_base = unsafe { mmio_base.add(caplength) };

        let hcsparams1 = unsafe {
            core::ptr::read_volatile(mmio_base.add(CAP_HCSPARAMS1) as *const u32)
        };
        let max_slots = (hcsparams1 & 0xFF) as usize;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as usize;

        let mut controller = Self {
            _mmio_base: mmio_base,
            op_base,
            max_ports,
            max_slots,
            cmd_ring: Dma::zeroed()?,
            _cmd_enqueue: 0,
            _cmd_cycle: true,
            dcbaa: Dma::zeroed()?,
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        // Ожидаем готовности контроллера (CNR == 0)
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_CNR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Останавливаем контроллер, если он работал
        let cmd = self.read_op32(OP_USBCMD);
        if (cmd & USBCMD_RS) != 0 {
            self.write_op32(OP_USBCMD, cmd & !USBCMD_RS);
            for _ in 0..100_000 {
                if (self.read_op32(OP_USBSTS) & USBSTS_HCH) != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Программный сброс xHCI контроллера
        self.write_op32(OP_USBCMD, USBCMD_HCRST);
        for _ in 0..100_000 {
            let status = self.read_op32(OP_USBCMD);
            if (status & USBCMD_HCRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Ожидаем завершения внутренней инициализации
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_CNR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Настраиваем количество активных слотов устройств
        self.write_op32(OP_CONFIG, self.max_slots as u32);

        // Настраиваем базовый массив контекстов устройств (DCBAAP)
        self.write_op64(OP_DCBAAP, self.dcbaa.physical() as u64);

        // Настраиваем кольцо команд (Command Ring) с Link TRB на последнем слоте
        let ring_phys = self.cmd_ring.physical() as u64;
        let last_idx = CMD_RING_SIZE - 1;
        self.cmd_ring[last_idx] = Trb::new(
            ring_phys,
            0,
            (TRB_TYPE_LINK << 10) | TRB_TC | TRB_CYCLE,
        );

        // Записываем адрес кольца команд и выставляем бит RCS = 1
        self.write_op64(OP_CRCR, ring_phys | 1);

        // Запускаем контроллер (Run/Stop = 1)
        self.write_op32(OP_USBCMD, USBCMD_RS | USBCMD_INTE);
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_HCH) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.scan_ports();
        Ok(())
    }

    /// Опрашивает физические порты xHCI на предмет подключенных устройств
    fn scan_ports(&self) {
        for port in 1..=self.max_ports {
            let port_reg = portsc_offset(port);
            let portsc = self.read_op32(port_reg);

            if (portsc & PORTSC_CCS) != 0 {
                let speed = (portsc >> 10) & 0x0F;
                let speed_str = match speed {
                    1 => "Full-Speed (12 Mbps)",
                    2 => "Low-Speed (1.5 Mbps)",
                    3 => "High-Speed (480 Mbps)",
                    4 => "SuperSpeed (5 Gbps)",
                    5 => "SuperSpeedPlus (10 Gbps)",
                    _ => "Unknown speed",
                };
                serial_println!("xhci: port {} device detected: {}", port, speed_str);
            }
        }
    }
}

/// Поиск и регистрация контроллеров USB 3.x xHCI (Class 0x0C, Subclass 0x03, ProgIF 0x30)
pub fn probe_pci(dev: &PciDevice, _dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30 {
        match XhciController::new(dev) {
            Ok(xhci) => {
                serial_println!(
                    "xhci: registered USB 3.x Host Controller ({:04x}:{:04x}, {} ports, {} slots)",
                    dev.vendor_id, dev.device_id, xhci.max_ports, xhci.max_slots
                );
                return true;
            }
            Err(e) => {
                serial_println!("xhci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
