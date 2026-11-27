pub mod context;
pub mod device;
pub mod regs;
pub mod ring;
pub mod trb;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::usb::host::{
    alloc_usb_bus_id, attach_enumerated_device, parse_config_interfaces, summarize_endpoints,
    UsbAttachedDevice, UsbEndpointMeta, UsbHostController, UsbTransferError,
};
use crate::i18n::t;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use device::XhciDeviceSlot;
use regs::*;
use ring::{EventRing, TrbRing};
use trb::{
    Trb, TRB_TYPE_ADDRESS_DEV, TRB_TYPE_CMD_COMPLETION, TRB_TYPE_CONFIG_EP, TRB_TYPE_ENABLE_SLOT,
};

/// Драйвер контроллера USB 3.0/3.1/3.2 xHCI (eXtensible Host Controller Interface)
pub struct XhciController {
    bus_id: u8,
    next_devnum: u8,
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
    active_slots: Vec<XhciDeviceSlot>,
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
        // Пишем 64-битные MMIO-регистры как пару 32-битных (lo -> hi) по xHCI 1.2 §5.3,
        // чтобы гарантировать совместимость с контроллерами без 64-бит атомарного MMIO.
        self.write_op32(offset, val as u32);
        self.write_op32(offset + 4, (val >> 32) as u32);
    }

    fn write_rt64(&self, offset: usize, val: u64) {
        unsafe {
            core::ptr::write_volatile(self.rt_base.add(offset) as *mut u32, val as u32);
            core::ptr::write_volatile(self.rt_base.add(offset + 4) as *mut u32, (val >> 32) as u32);
        }
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

        let dboff =
            unsafe { core::ptr::read_volatile(mmio_base.add(CAP_DBOFF) as *const u32) & !0x03 }
                as usize;
        let db_base = unsafe { mmio_base.add(dboff) };

        let rtsoff =
            unsafe { core::ptr::read_volatile(mmio_base.add(CAP_RTSOFF) as *const u32) & !0x1F }
                as usize;
        let rt_base = unsafe { mmio_base.add(rtsoff) };

        let mut controller = Self {
            bus_id: alloc_usb_bus_id(),
            next_devnum: 1,
            _mmio_base: mmio_base,
            op_base,
            rt_base,
            db_base,
            max_ports,
            max_slots,
            cmd_ring: TrbRing::new()?,
            event_ring: EventRing::new()?,
            dcbaa: Dma::zeroed()?,
            dma_bounce: Dma::zeroed()?,
            active_slots: Vec::new(),
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_CNR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if (self.read_op32(OP_USBSTS) & USBSTS_CNR) != 0 {
            return Err("xhci: controller not ready");
        }

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

        self.write_op32(OP_USBCMD, USBCMD_HCRST);
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBCMD) & USBCMD_HCRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if (self.read_op32(OP_USBCMD) & USBCMD_HCRST) != 0 {
            return Err("xhci: reset timeout");
        }

        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_CNR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.write_op32(OP_CONFIG, self.max_slots as u32);
        self.write_op64(OP_DCBAAP, self.dcbaa.physical() as u64);
        self.write_op64(OP_CRCR, self.cmd_ring.physical() | 1);

        // xHCI §5.5.2.3.2: ERSTSZ и ERDP обязаны быть настроены ДО записи ERSTBA (запуск кольца событий).
        unsafe {
            let rt = self.rt_base;
            core::ptr::write_volatile(rt.add(RT_ERSTSZ) as *mut u32, 1);
            core::ptr::write_volatile(rt.add(RT_IMAN) as *mut u32, 3);
        }
        self.write_rt64(RT_ERDP, self.event_ring.current_dequeue_phys() | (1 << 3));
        self.write_rt64(RT_ERSTBA, self.event_ring.erst_phys());

        self.write_op32(OP_USBCMD, USBCMD_RS | USBCMD_INTE);
        for _ in 0..100_000 {
            if (self.read_op32(OP_USBSTS) & USBSTS_HCH) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        Ok(())
    }

    fn execute_cmd(&mut self, trb: Trb) -> Result<Trb, &'static str> {
        let cmd_phys = self.cmd_ring.enqueue(trb);
        self.ring_host_doorbell();

        for _ in 0..2_000_000 {
            if let Some(ev) = self.event_ring.poll_event() {
                ring::advance_erdp(self.rt_base, self.event_ring.current_dequeue_phys());

                if ev.trb_type() == TRB_TYPE_CMD_COMPLETION && ev.data_ptr == cmd_phys {
                    let cc = (ev.status >> 24) & 0xFF;
                    if cc == CC_SUCCESS {
                        return Ok(ev);
                    }
                    return Err("xhci: command returned non-success code");
                }
            }
            core::hint::spin_loop();
        }

        Err("xhci: command execution timeout")
    }

    fn reset_port(&self, port: usize) -> Option<u8> {
        let port_reg = portsc_offset(port);
        let mut portsc = self.read_op32(port_reg);
        if (portsc & PORTSC_CCS) == 0 {
            return None;
        }

        let write_val = (portsc & 0x0E00_0E00) | PORTSC_PP | PORTSC_PR;
        self.write_op32(port_reg, write_val);
        for _ in 0..100_000 {
            portsc = self.read_op32(port_reg);
            if (portsc & PORTSC_PR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        if (portsc & PORTSC_PED) == 0 {
            return None;
        }
        Some(((portsc >> 10) & 0x0F) as u8)
    }

    /// Энумерирует подключённые USB устройства и настраивает их конечные точки через CONFIG_EP.
    pub fn enumerate_devices(&mut self) -> Vec<UsbAttachedDevice> {
        let mut attached = Vec::new();

        for port in 1..=self.max_ports {
            let Some(speed) = self.reset_port(port) else {
                continue;
            };

            let comp_ev = match self.execute_cmd(Trb::new(0, 0, TRB_TYPE_ENABLE_SLOT << 10)) {
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

            dev_slot.input_ctx.control.add_flags = 0x03;
            dev_slot.input_ctx.slot.init(port as u8, speed, 1);
            dev_slot.input_ctx.endpoints[0].init(
                4,
                dev_slot.max_packet_size,
                dev_slot.ep0_ring.physical(),
                0,
            );
            self.dcbaa[slot_id as usize] = dev_slot.device_ctx.physical() as u64;

            let addr_trb = Trb::new(
                dev_slot.input_ctx.physical() as u64,
                0,
                (TRB_TYPE_ADDRESS_DEV << 10) | ((slot_id as u32) << 24),
            );
            if let Err(e) = self.execute_cmd(addr_trb) {
                serial_println!("xhci: slot {} address device failed: {}", slot_id, e);
                continue;
            }

            let mut dev_desc = [0u8; 18];
            if dev_slot
                .get_descriptor(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    1,
                    0,
                    &mut dev_desc,
                    &mut self.dma_bounce,
                )
                .is_err()
            {
                serial_println!("xhci: slot {} get device descriptor failed", slot_id);
                continue;
            }

            let mut cfg_hdr = [0u8; 9];
            let mut raw_cfg = Vec::new();
            let mut interfaces = Vec::new();

            if dev_slot
                .get_descriptor(
                    &mut self.event_ring,
                    self.db_base,
                    self.rt_base,
                    2,
                    0,
                    &mut cfg_hdr,
                    &mut self.dma_bounce,
                )
                .is_ok()
            {
                let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize;
                let fetch_len = total_len.clamp(9, 512);
                raw_cfg.resize(fetch_len, 0);
                if dev_slot
                    .get_descriptor(
                        &mut self.event_ring,
                        self.db_base,
                        self.rt_base,
                        2,
                        0,
                        &mut raw_cfg,
                        &mut self.dma_bounce,
                    )
                    .is_ok()
                {
                    let cfg_val = cfg_hdr[5].max(1);
                    let _ = dev_slot.set_configuration(
                        &mut self.event_ring,
                        self.db_base,
                        self.rt_base,
                        cfg_val,
                        &mut self.dma_bounce,
                    );
                    interfaces = parse_config_interfaces(&raw_cfg);
                }
            }

            let all_eps: Vec<UsbEndpointMeta> = interfaces
                .iter()
                .flat_map(|i| i.endpoints.iter().copied())
                .collect();
            if !all_eps.is_empty() && dev_slot.prepare_configure_endpoints(&all_eps).is_ok() {
                let cfg_ep_trb = Trb::new(
                    dev_slot.input_ctx.physical() as u64,
                    0,
                    (TRB_TYPE_CONFIG_EP << 10) | ((slot_id as u32) << 24),
                );
                if let Err(e) = self.execute_cmd(cfg_ep_trb) {
                    serial_println!("xhci: slot {} CONFIG_EP failed: {}", slot_id, e);
                }
            }

            let (ep_bulk_in, ep_bulk_out, ep_intr_in) = summarize_endpoints(&all_eps);
            let devnum = self.next_devnum;
            self.next_devnum = self.next_devnum.saturating_add(1);

            let info = UsbAttachedDevice {
                bus: self.bus_id,
                devnum,
                slot_id,
                speed,
                vendor_id: u16::from_le_bytes([dev_desc[8], dev_desc[9]]),
                product_id: u16::from_le_bytes([dev_desc[10], dev_desc[11]]),
                dev_class: dev_desc[4],
                dev_subclass: dev_desc[5],
                dev_protocol: dev_desc[6],
                max_packet_size: dev_slot.max_packet_size,
                raw_config_desc: raw_cfg,
                interfaces,
                ep_bulk_in,
                ep_bulk_out,
                ep_intr_in,
            };

            self.active_slots.push(dev_slot);
            attached.push(info);
        }

        attached
    }

    fn slot_mut(&mut self, slot_id: u8) -> Result<&mut XhciDeviceSlot, UsbTransferError> {
        self.active_slots
            .iter_mut()
            .find(|s| s.slot_id == slot_id)
            .ok_or(UsbTransferError::NoDevice)
    }
}

impl UsbHostController for XhciController {
    fn bus_id(&self) -> u8 {
        self.bus_id
    }

    fn controller_name(&self) -> &'static str {
        "xhci"
    }

    fn control_transfer(
        &mut self,
        slot_id: u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        data: &mut [u8],
    ) -> Result<usize, UsbTransferError> {
        let db = self.db_base;
        let rt = self.rt_base;
        let slot_idx = self
            .active_slots
            .iter()
            .position(|s| s.slot_id == slot_id)
            .ok_or(UsbTransferError::NoDevice)?;
        self.active_slots[slot_idx].control_transfer(
            &mut self.event_ring,
            db,
            rt,
            bm_request_type,
            b_request,
            w_value,
            w_index,
            data,
            &mut self.dma_bounce,
        )
    }

    fn bulk_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        let db = self.db_base;
        let rt = self.rt_base;
        let slot_idx = self
            .active_slots
            .iter()
            .position(|s| s.slot_id == slot_id)
            .ok_or(UsbTransferError::NoDevice)?;
        self.active_slots[slot_idx].bulk_transfer(
            &mut self.event_ring,
            db,
            rt,
            ep_addr,
            data,
            &mut self.dma_bounce,
            timeout_ms,
        )
    }

    fn interrupt_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        let db = self.db_base;
        let rt = self.rt_base;
        let slot_idx = self
            .active_slots
            .iter()
            .position(|s| s.slot_id == slot_id)
            .ok_or(UsbTransferError::NoDevice)?;
        self.active_slots[slot_idx].interrupt_transfer(
            &mut self.event_ring,
            db,
            rt,
            ep_addr,
            data,
            timeout_ms,
        )
    }

    fn reset_device(&mut self, slot_id: u8) -> Result<(), UsbTransferError> {
        let port = self.slot_mut(slot_id)?.port as usize;
        self.reset_port(port)
            .map(|_| ())
            .ok_or(UsbTransferError::IoError)
    }
}

pub static XHCI_INSTANCES: Mutex<Vec<Arc<Mutex<XhciController>>>> = Mutex::new(Vec::new());

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30 {
        match XhciController::new(dev) {
            Ok(xhci) => {
                dev.driver_name = Some("xhci");
                let ports = xhci.max_ports;
                let bus = xhci.bus_id;
                let xhci_arc = Arc::new(Mutex::new(xhci));

                crate::klog!(
                    "xhci: {} ({:04x}:{:04x}, {} {}, bus {})",
                    t(
                        "активирован контроллер USB 3.x",
                        "registered USB 3.x Host Controller"
                    ),
                    dev.vendor_id,
                    dev.device_id,
                    ports,
                    t("портов", "ports"),
                    bus
                );

                // Энумерация освобождает mutex до регистрации, чтобы класс-драйверы могли вызывать control/bulk.
                let devices = xhci_arc.lock().enumerate_devices();
                let host_dyn: Arc<Mutex<dyn UsbHostController>> = xhci_arc.clone();

                for info in devices {
                    serial_println!(
                        "xhci: /dev/bus/usb/{:03}/{:03} {:04x}:{:04x} ({} interfaces)",
                        info.bus,
                        info.devnum,
                        info.vendor_id,
                        info.product_id,
                        info.interfaces.len()
                    );
                    attach_enumerated_device(&host_dyn, info, dm, block_count);
                }

                XHCI_INSTANCES.lock().push(xhci_arc);
                return true;
            }
            Err(e) => {
                serial_println!("xhci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
