pub mod regs;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::usb::host::{
    alloc_usb_bus_id, attach_enumerated_device, query_device_descriptors, UsbAttachedDevice,
    UsbHostController, UsbTransferError,
};
use crate::i18n::t;
use crate::serial_println;

use regs::*;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct UhciQh {
    head_link: u32,
    elem_link: u32,
    _pad: [u32; 2],
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct UhciTd {
    link: u32,
    ctrl_status: u32,
    token: u32,
    buffer: u32,
    _sw: [u32; 4],
}

struct UhciDeviceEntry {
    addr: u8,
    port: u8,
    low_speed: bool,
    max_pkt0: u16,
    toggles: [bool; 32],
}

pub struct UhciController {
    bus_id: u8,
    next_addr: u8,
    io_base: u16,
    frame_list: Dma<[u32; 1024]>,
    qh: Dma<UhciQh>,
    tds: Dma<[UhciTd; 32]>,
    setup_buf: Dma<[u8; 8]>,
    bounce: Dma<[u8; 4096]>,
    devices: Vec<UhciDeviceEntry>,
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

    fn out32(&self, reg: u16, val: u32) {
        let mut port = PortWriteOnly::<u32>::new(self.io_base + reg);
        unsafe { port.write(val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_io_space();
        pci.enable_bus_mastering();

        let bar4 = pci.bar(4).ok_or("uhci: BAR4 I/O not found")?;
        let io_base = bar4.address as u16;

        let mut frame_list: Dma<[u32; 1024]> = Dma::zeroed()?;
        let mut qh: Dma<UhciQh> = Dma::zeroed()?;
        let tds: Dma<[UhciTd; 32]> = Dma::zeroed()?;
        let setup_buf: Dma<[u8; 8]> = Dma::zeroed()?;
        let bounce: Dma<[u8; 4096]> = Dma::zeroed()?;

        let qh_phys = qh.physical();
        if qh_phys > u32::MAX as usize || frame_list.physical() > u32::MAX as usize {
            return Err("uhci: DMA buffer above 4GiB");
        }
        qh.head_link = 1;
        qh.elem_link = 1;
        for entry in frame_list.iter_mut() {
            *entry = (qh_phys as u32) | 0x02;
        }

        let mut controller = Self {
            bus_id: alloc_usb_bus_id(),
            next_addr: 1,
            io_base,
            frame_list,
            qh,
            tds,
            setup_buf,
            bounce,
            devices: Vec::new(),
        };
        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        self.out16(REG_USBCMD, USBCMD_HCRESET);
        for _ in 0..20_000 {
            if (self.in16(REG_USBCMD) & USBCMD_HCRESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.out16(REG_USBINTR, 0);
        self.out16(REG_FRNUM, 0);
        self.out32(REG_FLBASEADD, self.frame_list.physical() as u32);
        self.out16(REG_USBCMD, USBCMD_RS | USBCMD_CF | USBCMD_MAXP);
        Ok(())
    }

    fn reset_port(&self, port_idx: usize) -> Option<bool> {
        let reg = if port_idx == 0 { REG_PORTSC1 } else { REG_PORTSC2 };
        if (self.in16(reg) & PORTSC_CCS) == 0 {
            return None;
        }
        self.out16(reg, PORTSC_PR);
        for _ in 0..60_000 {
            core::hint::spin_loop();
        }
        self.out16(reg, 0);
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        let val = self.in16(reg);
        self.out16(reg, (val & !PORTSC_PR) | PORTSC_PED | PORTSC_CSC | PORTSC_PEDC);
        for _ in 0..20_000 {
            let st = self.in16(reg);
            if (st & PORTSC_PED) != 0 {
                return Some((st & PORTSC_LS) != 0);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn build_td(pid: u8, addr: u8, ep: u8, toggle: bool, len: usize, low_speed: bool, buf_phys: u32) -> UhciTd {
        let max_len = if len == 0 { 0x7FF } else { (len - 1) as u32 };
        let ls_bit = if low_speed { 1 << 26 } else { 0 };
        let token = (pid as u32)
            | ((addr as u32 & 0x7F) << 8)
            | ((ep as u32 & 0x0F) << 15)
            | ((toggle as u32) << 19)
            | (max_len << 21);
        UhciTd {
            link: 1,
            ctrl_status: (1 << 23) | (3 << 27) | ls_bit,
            token,
            buffer: buf_phys,
            _sw: [0; 4],
        }
    }

    fn run_td_chain(&mut self, count: usize, timeout_ms: u32) -> Result<(), UsbTransferError> {
        if count == 0 {
            return Ok(());
        }
        let base_phys = self.tds.physical() as u32;
        let stride = core::mem::size_of::<UhciTd>() as u32;
        for i in 0..count {
            self.tds[i].link = if i + 1 < count {
                (base_phys + ((i + 1) as u32) * stride) | (1 << 2)
            } else {
                1
            };
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.qh.elem_link = base_phys;

        let spins = (timeout_ms.max(20) as usize).saturating_mul(2_500);
        for _ in 0..spins {
            let mut active = false;
            for i in 0..count {
                let st = unsafe { core::ptr::read_volatile(&self.tds[i].ctrl_status) };
                if (st & (1 << 22)) != 0 {
                    self.qh.elem_link = 1;
                    return Err(UsbTransferError::Stall);
                }
                if (st & (1 << 23)) != 0 {
                    active = true;
                    break;
                }
            }
            if !active {
                self.qh.elem_link = 1;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        self.qh.elem_link = 1;
        Err(UsbTransferError::Timeout)
    }

    fn device_params(&self, addr: u8) -> (bool, u16) {
        self.devices
            .iter()
            .find(|d| d.addr == addr)
            .map(|d| (d.low_speed, d.max_pkt0))
            .unwrap_or((false, 8))
    }

    pub fn enumerate_devices(&mut self) -> Vec<UsbAttachedDevice> {
        let mut attached = Vec::new();
        for port in 0..2 {
            let Some(low_speed) = self.reset_port(port) else {
                continue;
            };
            let mps0 = if low_speed { 8 } else { 64 };
            self.devices.push(UhciDeviceEntry {
                addr: 0,
                port: port as u8,
                low_speed,
                max_pkt0: mps0,
                toggles: [false; 32],
            });
            let addr = self.next_addr;
            self.next_addr = self.next_addr.saturating_add(1);
            if self.control_transfer(0, 0x00, 5, addr as u16, 0, &mut []).is_err() {
                self.devices.retain(|d| d.addr != 0);
                continue;
            }
            self.devices.retain(|d| d.addr != 0);
            self.devices.push(UhciDeviceEntry {
                addr,
                port: port as u8,
                low_speed,
                max_pkt0: mps0,
                toggles: [false; 32],
            });

            let speed = if low_speed { 1 } else { 2 };
            if let Some(info) = query_device_descriptors(self, self.bus_id, addr, addr, speed, mps0) {
                if let Some(entry) = self.devices.iter_mut().find(|d| d.addr == addr) {
                    entry.max_pkt0 = info.max_packet_size;
                }
                attached.push(info);
            }
        }
        attached
    }
}

impl UsbHostController for UhciController {
    fn bus_id(&self) -> u8 {
        self.bus_id
    }

    fn controller_name(&self) -> &'static str {
        "uhci"
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
        if data.len() > self.bounce.len() {
            return Err(UsbTransferError::IoError);
        }
        let (low_speed, mps0) = self.device_params(slot_id);
        let is_in = (bm_request_type & 0x80) != 0;
        let mps = mps0.max(8) as usize;

        self.setup_buf[0] = bm_request_type;
        self.setup_buf[1] = b_request;
        self.setup_buf[2..4].copy_from_slice(&w_value.to_le_bytes());
        self.setup_buf[4..6].copy_from_slice(&w_index.to_le_bytes());
        self.setup_buf[6..8].copy_from_slice(&(data.len() as u16).to_le_bytes());

        if !is_in && !data.is_empty() {
            self.bounce[..data.len()].copy_from_slice(data);
        }

        let mut td_idx = 0usize;
        self.tds[td_idx] = Self::build_td(
            PID_SETUP,
            slot_id,
            0,
            false,
            8,
            low_speed,
            self.setup_buf.physical() as u32,
        );
        td_idx += 1;

        let mut toggle = true;
        let mut offset = 0usize;
        let bounce_phys = self.bounce.physical() as u32;
        while offset < data.len() && td_idx < 30 {
            let chunk = (data.len() - offset).min(mps);
            let pid = if is_in { PID_IN } else { PID_OUT };
            self.tds[td_idx] = Self::build_td(
                pid,
                slot_id,
                0,
                toggle,
                chunk,
                low_speed,
                bounce_phys + offset as u32,
            );
            td_idx += 1;
            toggle = !toggle;
            offset += chunk;
        }

        let status_pid = if data.is_empty() || !is_in { PID_IN } else { PID_OUT };
        self.tds[td_idx] = Self::build_td(status_pid, slot_id, 0, true, 0, low_speed, 0);
        td_idx += 1;

        self.run_td_chain(td_idx, 100)?;
        if is_in && !data.is_empty() {
            data.copy_from_slice(&self.bounce[..data.len()]);
        }
        Ok(data.len())
    }

    fn bulk_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        if data.len() > self.bounce.len() {
            return Err(UsbTransferError::IoError);
        }
        let (low_speed, _) = self.device_params(slot_id);
        let is_in = (ep_addr & 0x80) != 0;
        let ep = ep_addr & 0x0F;
        let toggle_slot = ((ep as usize) * 2 + is_in as usize).min(31);
        let mut toggle = self
            .devices
            .iter()
            .find(|d| d.addr == slot_id)
            .map(|d| d.toggles[toggle_slot])
            .unwrap_or(false);

        if !is_in {
            self.bounce[..data.len()].copy_from_slice(data);
        }

        let mut offset = 0usize;
        let mut td_idx = 0usize;
        let pid = if is_in { PID_IN } else { PID_OUT };
        let bounce_phys = self.bounce.physical() as u32;
        while offset < data.len() && td_idx < 31 {
            let chunk = (data.len() - offset).min(64);
            self.tds[td_idx] = Self::build_td(
                pid,
                slot_id,
                ep,
                toggle,
                chunk,
                low_speed,
                bounce_phys + offset as u32,
            );
            td_idx += 1;
            toggle = !toggle;
            offset += chunk;
        }
        if data.is_empty() {
            self.tds[0] = Self::build_td(pid, slot_id, ep, toggle, 0, low_speed, 0);
            td_idx = 1;
            toggle = !toggle;
        }

        self.run_td_chain(td_idx, timeout_ms)?;
        if let Some(d) = self.devices.iter_mut().find(|d| d.addr == slot_id) {
            d.toggles[toggle_slot] = toggle;
        }
        if is_in && !data.is_empty() {
            data.copy_from_slice(&self.bounce[..data.len()]);
        }
        Ok(data.len())
    }

    fn interrupt_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        self.bulk_transfer(slot_id, ep_addr, data, timeout_ms.max(1))
    }

    fn reset_device(&mut self, slot_id: u8) -> Result<(), UsbTransferError> {
        let port = self
            .devices
            .iter()
            .find(|d| d.addr == slot_id)
            .map(|d| d.port as usize)
            .ok_or(UsbTransferError::NoDevice)?;
        self.reset_port(port)
            .map(|_| ())
            .ok_or(UsbTransferError::IoError)
    }
}

pub static UHCI_INSTANCES: Mutex<Vec<Arc<Mutex<UhciController>>>> = Mutex::new(Vec::new());

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x00 {
        match UhciController::new(dev) {
            Ok(uhci) => {
                dev.driver_name = Some("uhci");
                let bus = uhci.bus_id;
                let arc = Arc::new(Mutex::new(uhci));
                crate::klog!(
                    "uhci: {} ({:04x}:{:04x}, bus {})",
                    t(
                        "активирован контроллер USB 1.1 UHCI",
                        "registered USB 1.1 UHCI Host Controller"
                    ),
                    dev.vendor_id,
                    dev.device_id,
                    bus
                );
                let attached = arc.lock().enumerate_devices();
                let host_dyn: Arc<Mutex<dyn UsbHostController>> = arc.clone();
                for info in attached {
                    attach_enumerated_device(&host_dyn, info, dm, block_count);
                }
                UHCI_INSTANCES.lock().push(arc);
                return true;
            }
            Err(e) => {
                serial_println!("uhci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
