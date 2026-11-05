pub mod context;
pub mod device;
pub mod regs;
pub mod ring;
pub mod trb;

use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::BlockDevice;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use device::XhciDeviceSlot;
use regs::*;
use ring::{EventRing, TrbRing};
use trb::{Trb, TRB_TYPE_ADDRESS_DEV, TRB_TYPE_CMD_COMPLETION, TRB_TYPE_ENABLE_SLOT};

/// Драйвер контроллера USB 3.0/3.1/3.2 xHCI (eXtensible Host Controller Interface)
pub struct XhciController {
    _mmio_base: *mut u8,
    op_base: *mut u8,
    rt_base: *mut u8,
    db_base: *mut u8,
    max_ports: usize,
    max_slots: usize,
    cmd_ring: TrbRing,
    event_ring: EventRing,
    dcbaa: Dma<[u64; 256]>,
    dma_bounce: Dma<[u8; 4096]>,
    active_slots: alloc::vec::Vec<XhciDeviceSlot>,
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

    fn ring_host_doorbell(&self) {
        unsafe {
            core::ptr::write_volatile(self.db_base as *mut u32, 0);
        }
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

        let dboff = unsafe { core::ptr::read_volatile(mmio_base.add(CAP_DBOFF) as *const u32) & !0x03 } as usize;
        let db_base = unsafe { mmio_base.add(dboff) };

        let rtsoff = unsafe { core::ptr::read_volatile(mmio_base.add(CAP_RTSOFF) as *const u32) & !0x1F } as usize;
        let rt_base = unsafe { mmio_base.add(rtsoff) };

        let cmd_ring = TrbRing::new()?;
        let event_ring = EventRing::new()?;
        let dcbaa = Dma::zeroed()?;
        let dma_bounce = Dma::zeroed()?;

        let mut controller = Self {
            _mmio_base: mmio_base,
            op_base,
            rt_base,
            db_base,
            max_ports,
            max_slots,
            cmd_ring,
            event_ring,
            dcbaa,
            dma_bounce,
            active_slots: alloc::vec::Vec::new(),
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

        // Останавливаем контроллер при необходимости
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

        // Программный сброс xHCI контроллера (HCRST)
        self.write_op32(OP_USBCMD, USBCMD_HCRST);
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBCMD) & USBCMD_HCRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_CNR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Настройка количества активных слотов устройств
        self.write_op32(OP_CONFIG, self.max_slots as u32);

        // Настройка базового массива контекстов устройств (DCBAAP)
        self.write_op64(OP_DCBAAP, self.dcbaa.physical() as u64);

        // Настройка кольца команд (CRCR) с битом RCS = 1
        self.write_op64(OP_CRCR, self.cmd_ring.physical() | 1);

        // Настройка рантайм-прерываний (Interrupter 0)
        unsafe {
            let rt = self.rt_base;
            let erstba_ptr = rt.add(RT_ERSTBA) as *mut u64;
            core::ptr::write_volatile(erstba_ptr, self.event_ring.erst_phys());

            let erstsz_ptr = rt.add(RT_ERSTSZ) as *mut u32;
            core::ptr::write_volatile(erstsz_ptr, 1);

            let erdp_ptr = rt.add(RT_ERDP) as *mut u64;
            core::ptr::write_volatile(erdp_ptr, self.event_ring.current_dequeue_phys() | (1 << 3));

            let iman_ptr = rt.add(RT_IMAN) as *mut u32;
            core::ptr::write_volatile(iman_ptr, 3); // IP | IE
        }

        // Запуск контроллера
        self.write_op32(OP_USBCMD, USBCMD_RS | USBCMD_INTE);
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_HCH) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        Ok(())
    }

    /// Отправляет команду хост-контроллеру и ожидает её завершения по Event Ring
    fn execute_cmd(&mut self, trb: Trb) -> Result<Trb, &'static str> {
        let cmd_phys = self.cmd_ring.enqueue(trb);
        self.ring_host_doorbell();

        for _ in 0..2_000_000 {
            if let Some(ev) = self.event_ring.poll_event() {
                unsafe {
                    let erdp_ptr = self.rt_base.add(RT_ERDP) as *mut u64;
                    core::ptr::write_volatile(erdp_ptr, self.event_ring.current_dequeue_phys() | (1 << 3));
                }

                if ev.trb_type() == TRB_TYPE_CMD_COMPLETION && ev.data_ptr == cmd_phys {
                    let cc = (ev.status >> 24) & 0xFF;
                    if cc == CC_SUCCESS {
                        return Ok(ev);
                    } else {
                        return Err("xhci: command returned non-success code");
                    }
                }
            }
            core::hint::spin_loop();
        }

        Err("xhci: command execution timeout")
    }

    /// Энумерация подключенных устройств на портах xHCI и их регистрация в DeviceManager
    pub fn probe_devices(&mut self, dm: &mut DeviceManager, block_count: &mut usize) {
        for port in 1..=self.max_ports {
            let port_reg = portsc_offset(port);
            let mut portsc = self.read_op32(port_reg);

            if (portsc & PORTSC_CCS) == 0 {
                continue;
            }

            // Маскируем RW1C биты, сохраняем питание порта PP и взводим PR
            let write_val = (portsc & 0x0E00_0E00) | PORTSC_PP | PORTSC_PR;
            self.write_op32(port_reg, write_val);
            for _ in 0..100_000 {
                portsc = self.read_op32(port_reg);
                if (portsc & PORTSC_PR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            let speed = ((portsc >> 10) & 0x0F) as u8;

            // 1. Команда Enable Slot
            let enable_trb = Trb::new(0, 0, TRB_TYPE_ENABLE_SLOT << 10);
            let comp_ev = match self.execute_cmd(enable_trb) {
                Ok(ev) => ev,
                Err(e) => {
                    serial_println!("xhci: port {} enable slot failed: {}", port, e);
                    continue;
                }
            };

            let slot_id = ((comp_ev.control >> 24) & 0xFF) as u8;
            let mut dev_slot = match XhciDeviceSlot::new(slot_id, port as u8, speed) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // 2. Настройка Input Context
            dev_slot.input_ctx.control.add_flags = 0x03; // Slot Context + EP0
            dev_slot.input_ctx.slot.init(port as u8, speed, 1);
            dev_slot.input_ctx.endpoints[0].init(
                4, // Control Bi-directional
                dev_slot.max_packet_size,
                dev_slot.ep0_ring.physical(),
                0,
            );

            // Регистрация Device Context в DCBAA
            self.dcbaa[slot_id as usize] = dev_slot.device_ctx.physical() as u64;

            // 3. Команда Address Device (BSR = 0)
            let addr_trb = Trb::new(
                dev_slot.input_ctx.physical() as u64,
                0,
                (TRB_TYPE_ADDRESS_DEV << 10) | ((slot_id as u32) << 24),
            );
            if let Err(e) = self.execute_cmd(addr_trb) {
                serial_println!("xhci: slot {} address device failed: {}", slot_id, e);
                continue;
            }

            // 4. Чтение Device Descriptor (18 байт)
            let mut dev_desc = [0u8; 18];
            if dev_slot
                .get_descriptor(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    1, // DEVICE
                    0,
                    &mut dev_desc,
                    &mut self.dma_bounce,
                )
                .is_err()
            {
                serial_println!("xhci: slot {} get device descriptor failed", slot_id);
                continue;
            }

            // 5. Чтение Configuration Descriptor
            let mut cfg_hdr = [0u8; 9];
            if dev_slot
                .get_descriptor(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    2, // CONFIGURATION
                    0,
                    &mut cfg_hdr,
                    &mut self.dma_bounce,
                )
                .is_ok()
            {
                let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize;
                let mut full_cfg = [0u8; 256];
                let fetch_len = total_len.min(256);
                let _ = dev_slot.get_descriptor(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    2,
                    0,
                    &mut full_cfg[..fetch_len],
                    &mut self.dma_bounce,
                );

                let _ = dev_slot.set_configuration(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    1,
                    &mut self.dma_bounce,
                );

                // Анализ дескрипторов интерфейсов
                self.parse_and_attach_interfaces(
                    &full_cfg[..fetch_len],
                    &mut dev_slot,
                    dm,
                    block_count,
                );
            }

            self.active_slots.push(dev_slot);
        }
    }

    fn parse_and_attach_interfaces(
        &mut self,
        cfg_data: &[u8],
        slot: &mut XhciDeviceSlot,
        dm: &mut DeviceManager,
        block_count: &mut usize,
    ) {
        let mut idx = 0;
        while idx + 2 <= cfg_data.len() {
            let len = cfg_data[idx] as usize;
            if len == 0 || idx + len > cfg_data.len() {
                break;
            }
            let desc_type = cfg_data[idx + 1];

            // Interface Descriptor (type 4, len 9)
            if desc_type == 4 && len >= 9 {
                let if_class = cfg_data[idx + 5];
                let _if_subclass = cfg_data[idx + 6];
                let if_proto = cfg_data[idx + 7];

                if if_class == 0x08 {
                    // USB Mass Storage
                    let letter = (b'a' + (*block_count as u8).min(25)) as char;
                    let dev_name = format!("sd{}", letter);
                    let mut storage = match super::classes::storage::UsbStorageDevice::new(slot.slot_id, 3, 2) {
                        Ok(st) => st,
                        Err(_) => { idx += len; continue; }
                    };
                    let _ = storage.init_storage(slot, &mut self.event_ring, self.db_base, self.rt_base);
                    let cap_mb = (storage.sector_count() * storage.sector_size() as u64) / (1024 * 1024);
                    serial_println!(
                        "xhci: registered /dev/{} (USB Mass Storage, {} MB)",
                        dev_name, cap_mb
                    );
                    dm.register_block_device(dev_name, Arc::new(Mutex::new(storage)));
                    *block_count += 1;
                } else if if_class == 0x03 {
                    // USB HID
                    if if_proto == 0x01 {
                        serial_println!("xhci: registered USB Boot Keyboard");
                    } else if if_proto == 0x02 {
                        let mouse = super::classes::hid::UsbMouseDevice::new();
                        serial_println!("xhci: registered USB Optical Mouse");
                        dm.register_input_device(Arc::new(Mutex::new(mouse)));
                    }
                }
            }
            idx += len;
        }
    }
}

pub static XHCI_INSTANCES: Mutex<alloc::vec::Vec<XhciController>> = Mutex::new(alloc::vec::Vec::new());

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30 {
        match XhciController::new(dev) {
            Ok(mut xhci) => {
                dev.driver_name = Some("xhci");
                serial_println!(
                    "xhci: registered USB 3.x Host Controller ({:04x}:{:04x}, {} ports)",
                    dev.vendor_id, dev.device_id, xhci.max_ports
                );
                xhci.probe_devices(dm, block_count);
                XHCI_INSTANCES.lock().push(xhci);
                return true;
            }
            Err(e) => {
                serial_println!("xhci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
