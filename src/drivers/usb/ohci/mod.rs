pub mod regs;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::usb::host::{
    alloc_usb_bus_id, attach_enumerated_device, query_device_descriptors, UsbAttachedDevice,
    UsbHostController, UsbTransferError,
};
use crate::i18n::t;
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use regs::*;

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

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct OhciEd {
    flags: u32,
    tail_p: u32,
    head_p: u32,
    next_ed: u32,
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct OhciTd {
    flags: u32,
    cbp: u32,
    next_td: u32,
    be: u32,
}

struct OhciDeviceEntry {
    addr: u8,
    port: u8,
    low_speed: bool,
    max_pkt0: u16,
    toggles: [bool; 32],
}

pub struct OhciController {
    bus_id: u8,
    next_addr: u8,
    mmio_base: *mut u8,
    port_count: usize,
    hcca: Dma<OhciHcca>,
    ed: Dma<OhciEd>,
    tds: Dma<[OhciTd; 16]>,
    setup_buf: Dma<[u8; 8]>,
    bounce: Dma<[u8; 4096]>,
    devices: Vec<OhciDeviceEntry>,
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

        let desc_a =
            unsafe { core::ptr::read_volatile(mmio_base.add(REG_RH_DESCRIPTOR_A) as *const u32) };
        let port_count = ((desc_a & 0xFF) as usize).min(15);

        let mut controller = Self {
            bus_id: alloc_usb_bus_id(),
            next_addr: 1,
            mmio_base,
            port_count,
            hcca: Dma::zeroed()?,
            ed: Dma::zeroed()?,
            tds: Dma::zeroed()?,
            setup_buf: Dma::zeroed()?,
            bounce: Dma::zeroed()?,
            devices: Vec::new(),
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
        self.write_reg32(REG_COMMAND_STATUS, CMD_HCR);
        for _ in 0..20_000 {
            if (self.read_reg32(REG_COMMAND_STATUS) & CMD_HCR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.write_reg32(REG_HCCA, self.hcca.physical() as u32);
        self.write_reg32(REG_CONTROL_HEAD_ED, 0);
        self.write_reg32(REG_BULK_HEAD_ED, 0);

        let fminterval = 0x2EDF | (((0x2EDF - 210) * 6 / 7) << 16);
        self.write_reg32(REG_FM_INTERVAL, fminterval);
        self.write_reg32(REG_PERIODIC_START, (0x2EDF * 9) / 10);

        self.write_reg32(
            REG_CONTROL,
            CONTROL_HCFS_OPERATIONAL | CONTROL_PLE | CONTROL_CLE | CONTROL_BLE,
        );

        for p in 0..self.port_count {
            let offset = REG_RH_PORT_STATUS_BASE + (p * 4);
            self.write_reg32(offset, RH_PORT_PPS);
        }

        Ok(())
    }

    fn reset_port(&self, p: usize) -> Option<bool> {
        let offset = REG_RH_PORT_STATUS_BASE + (p * 4);
        if (self.read_reg32(offset) & RH_PORT_CCS) == 0 {
            return None;
        }
        self.write_reg32(offset, RH_PORT_PRS);
        for _ in 0..50_000 {
            let st = self.read_reg32(offset);
            if (st & RH_PORT_PRS) == 0 && (st & RH_PORT_PES) != 0 {
                self.write_reg32(offset, RH_PORT_PRSC | RH_PORT_CSC);
                return Some((st & RH_PORT_LSDA) != 0);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn execute_td_list(
        &mut self,
        is_bulk: bool,
        addr: u8,
        ep: u8,
        mps: u16,
        low_speed: bool,
        count: usize,
        timeout_ms: u32,
    ) -> Result<(), UsbTransferError> {
        let td_base = self.tds.physical() as u32;
        let stride = core::mem::size_of::<OhciTd>() as u32;
        for i in 0..count {
            self.tds[i].next_td = td_base + ((i + 1) as u32) * stride;
        }
        // Завершающий dummy TD для HcED->TailP
        self.tds[count] = OhciTd::default();
        let tail_phys = td_base + (count as u32) * stride;

        let ls_bit = if low_speed { 1 << 13 } else { 0 };
        self.ed.flags =
            (addr as u32 & 0x7F) | ((ep as u32 & 0xF) << 7) | ls_bit | ((mps as u32) << 16);
        self.ed.head_p = td_base;
        self.ed.tail_p = tail_phys;
        self.ed.next_ed = 0;

        let ed_phys = self.ed.physical() as u32;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        if is_bulk {
            self.write_reg32(REG_BULK_HEAD_ED, ed_phys);
            self.write_reg32(REG_BULK_CURRENT_ED, 0);
            self.write_reg32(REG_COMMAND_STATUS, CMD_BLF);
        } else {
            self.write_reg32(REG_CONTROL_HEAD_ED, ed_phys);
            self.write_reg32(REG_CONTROL_CURRENT_ED, 0);
            self.write_reg32(REG_COMMAND_STATUS, CMD_CLF);
        }

        let spins = (timeout_ms.max(20) as usize).saturating_mul(2_500);
        for _ in 0..spins {
            let last_flags = unsafe { core::ptr::read_volatile(&self.tds[count - 1].flags) };
            let cc = (last_flags >> 28) & 0xF;
            if cc != 0xF {
                self.ed.flags |= 1 << 14; // Skip bit после завершения
                return if cc == 0 || cc == 9 {
                    Ok(())
                } else if cc == 4 {
                    Err(UsbTransferError::Stall)
                } else {
                    Err(UsbTransferError::IoError)
                };
            }
            core::hint::spin_loop();
        }
        self.ed.flags |= 1 << 14;
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
        for p in 0..self.port_count {
            let Some(low_speed) = self.reset_port(p) else {
                continue;
            };
            let mps0 = if low_speed { 8 } else { 64 };
            self.devices.push(OhciDeviceEntry {
                addr: 0,
                port: p as u8,
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
            self.devices.push(OhciDeviceEntry {
                addr,
                port: p as u8,
                low_speed,
                max_pkt0: mps0,
                toggles: [false; 32],
            });
            let speed = if low_speed { 1 } else { 2 };
            if let Some(info) = query_device_descriptors(self, self.bus_id, addr, addr, speed, mps0)
            {
                if let Some(d) = self.devices.iter_mut().find(|d| d.addr == addr) {
                    d.max_pkt0 = info.max_packet_size;
                }
                attached.push(info);
            }
        }
        attached
    }
}

impl UsbHostController for OhciController {
    fn bus_id(&self) -> u8 {
        self.bus_id
    }

    fn controller_name(&self) -> &'static str {
        "ohci"
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

        self.setup_buf[0] = bm_request_type;
        self.setup_buf[1] = b_request;
        self.setup_buf[2..4].copy_from_slice(&w_value.to_le_bytes());
        self.setup_buf[4..6].copy_from_slice(&w_index.to_le_bytes());
        self.setup_buf[6..8].copy_from_slice(&(data.len() as u16).to_le_bytes());

        if !is_in && !data.is_empty() {
            self.bounce[..data.len()].copy_from_slice(data);
        }

        let setup_phys = self.setup_buf.physical() as u32;
        let mut idx = 0usize;
        // Setup TD (DP=0, DATA0 toggle=2, CC=0xF)
        self.tds[idx] = OhciTd {
            flags: (0xF << 28) | (2 << 24) | (7 << 21),
            cbp: setup_phys,
            next_td: 0,
            be: setup_phys + 7,
        };
        idx += 1;

        if !data.is_empty() {
            let dp = if is_in { 2 } else { 1 };
            let phys = self.bounce.physical() as u32;
            // Data TD (toggle DATA1=3, rounding=1<<18)
            self.tds[idx] = OhciTd {
                flags: (0xF << 28) | (3 << 24) | (7 << 21) | (dp << 19) | (1 << 18),
                cbp: phys,
                next_td: 0,
                be: phys + (data.len() as u32 - 1),
            };
            idx += 1;
        }

        let st_dp = if data.is_empty() || !is_in { 2 } else { 1 };
        self.tds[idx] = OhciTd {
            flags: (0xF << 28) | (3 << 24) | (7 << 21) | (st_dp << 19),
            cbp: 0,
            next_td: 0,
            be: 0,
        };
        idx += 1;

        self.execute_td_list(false, slot_id, 0, mps0, low_speed, idx, 100)?;
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
        let toggle = self
            .devices
            .iter()
            .find(|d| d.addr == slot_id)
            .map(|d| d.toggles[toggle_slot])
            .unwrap_or(false);
        if !is_in && !data.is_empty() {
            self.bounce[..data.len()].copy_from_slice(data);
        }
        let dp = if is_in { 2 } else { 1 };
        let td_toggle = if toggle { 3 } else { 2 };
        let phys = if data.is_empty() {
            0
        } else {
            self.bounce.physical() as u32
        };
        let be = if data.is_empty() {
            0
        } else {
            phys + (data.len() as u32 - 1)
        };
        self.tds[0] = OhciTd {
            flags: (0xF << 28) | (td_toggle << 24) | (7 << 21) | (dp << 19) | (1 << 18),
            cbp: phys,
            next_td: 0,
            be,
        };
        self.execute_td_list(true, slot_id, ep, 64, low_speed, 1, timeout_ms)?;
        let next_toggle = unsafe { (core::ptr::read_volatile(&self.ed.head_p) & 0x02) != 0 };
        if let Some(d) = self.devices.iter_mut().find(|d| d.addr == slot_id) {
            d.toggles[toggle_slot] = next_toggle;
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

pub static OHCI_INSTANCES: Mutex<Vec<Arc<Mutex<OhciController>>>> = Mutex::new(Vec::new());

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x10 {
        match OhciController::new(dev) {
            Ok(ohci) => {
                dev.driver_name = Some("ohci");
                let ports = ohci.port_count;
                let bus = ohci.bus_id;
                let arc = Arc::new(Mutex::new(ohci));
                crate::klog!(
                    "ohci: {} ({:04x}:{:04x}, {} {}, bus {})",
                    t(
                        "активирован контроллер USB 1.1 OHCI",
                        "registered USB 1.1 OHCI Host Controller"
                    ),
                    dev.vendor_id,
                    dev.device_id,
                    ports,
                    t("портов", "ports"),
                    bus
                );
                let attached = arc.lock().enumerate_devices();
                let host_dyn: Arc<Mutex<dyn UsbHostController>> = arc.clone();
                for info in attached {
                    attach_enumerated_device(&host_dyn, info, dm, block_count);
                }
                OHCI_INSTANCES.lock().push(arc);
                return true;
            }
            Err(e) => {
                serial_println!("ohci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
