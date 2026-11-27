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

#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct EhciQtd {
    next_qtd: u32,
    alt_next_qtd: u32,
    token: u32,
    buf_ptrs: [u32; 5],
    _ext: [u32; 8],
}

#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct EhciQh {
    horiz_link: u32,
    ep_chars: u32,
    ep_caps: u32,
    cur_qtd: u32,
    next_qtd: u32,
    alt_next_qtd: u32,
    token: u32,
    buf_ptrs: [u32; 5],
    _pad: [u32; 4],
}

struct EhciDeviceEntry {
    addr: u8,
    port: u8,
    max_pkt0: u16,
    toggles: [bool; 32],
}

pub struct EhciController {
    bus_id: u8,
    next_addr: u8,
    _mmio_base: *mut u8,
    op_base: *mut u8,
    port_count: usize,
    head_qh: Dma<EhciQh>,
    work_qh: Dma<EhciQh>,
    qtds: Dma<[EhciQtd; 8]>,
    setup_buf: Dma<[u8; 8]>,
    bounce: Dma<[u8; 4096]>,
    devices: Vec<EhciDeviceEntry>,
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

        let hcsparams =
            unsafe { core::ptr::read_volatile(mmio_base.add(CAP_HCSPARAMS) as *const u32) };
        let port_count = ((hcsparams & 0x0F) as usize).min(15);

        let mut head_qh: Dma<EhciQh> = Dma::zeroed()?;
        let head_phys = head_qh.physical() as u32;
        head_qh.horiz_link = head_phys | 0x02;
        head_qh.ep_chars = 1 << 15; // H = Head of Reclamation List
        head_qh.next_qtd = 1;
        head_qh.alt_next_qtd = 1;
        head_qh.token = 1 << 6; // Halted

        let mut controller = Self {
            bus_id: alloc_usb_bus_id(),
            next_addr: 1,
            _mmio_base: mmio_base,
            op_base,
            port_count,
            head_qh,
            work_qh: Dma::zeroed()?,
            qtds: Dma::zeroed()?,
            setup_buf: Dma::zeroed()?,
            bounce: Dma::zeroed()?,
            devices: Vec::new(),
        };

        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&mut self) -> Result<(), &'static str> {
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

        self.write_op32(OP_USBCMD, USBCMD_HCRESET);
        for _ in 0..50_000 {
            if (self.read_op32(OP_USBCMD) & USBCMD_HCRESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.write_op32(OP_CTRLDSSEGMENT, 0);
        self.write_op32(OP_PERIODICLISTBASE, 0);
        self.write_op32(OP_ASYNCLISTADDR, self.head_qh.physical() as u32);
        self.write_op32(OP_CONFIGFLAG, 1);

        for port in 0..self.port_count {
            let reg = OP_PORTSC_BASE + (port * 4);
            let val = self.read_op32(reg);
            self.write_op32(reg, val | PORTSC_POWER);
        }

        self.write_op32(OP_USBCMD, USBCMD_RS | USBCMD_ASE);
        Ok(())
    }

    fn reset_port(&self, port: usize) -> bool {
        let reg = OP_PORTSC_BASE + (port * 4);
        let st = self.read_op32(reg);
        if (st & PORTSC_CONNECT) == 0 {
            return false;
        }
        // Low-speed устройства передаём companion-контроллеру OHCI/UHCI
        if (st & PORTSC_LINE_MASK) == PORTSC_LINE_K {
            self.write_op32(reg, st | PORTSC_OWNER);
            return false;
        }
        self.write_op32(reg, (st & !PORTSC_ENABLE) | PORTSC_RESET);
        for _ in 0..60_000 {
            core::hint::spin_loop();
        }
        let st2 = self.read_op32(reg);
        self.write_op32(reg, st2 & !PORTSC_RESET);
        for _ in 0..20_000 {
            let after = self.read_op32(reg);
            if (after & PORTSC_RESET) == 0 {
                return (after & PORTSC_ENABLE) != 0;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn run_qtd_chain(
        &mut self,
        addr: u8,
        ep: u8,
        mps: u16,
        count: usize,
        timeout_ms: u32,
    ) -> Result<(), UsbTransferError> {
        let qtd_base = self.qtds.physical() as u32;
        let stride = core::mem::size_of::<EhciQtd>() as u32;
        for i in 0..count {
            self.qtds[i].next_qtd = if i + 1 < count {
                qtd_base + ((i + 1) as u32) * stride
            } else {
                1
            };
            self.qtds[i].alt_next_qtd = 1;
        }

        let head_phys = self.head_qh.physical() as u32;
        let work_phys = self.work_qh.physical() as u32;
        *self.work_qh = EhciQh {
            horiz_link: head_phys | 0x02,
            // eps = 2 (High-Speed), max_pkt, addr, ep, dtc = 1
            ep_chars: (addr as u32 & 0x7F)
                | ((ep as u32 & 0xF) << 8)
                | (2 << 12)
                | (1 << 14)
                | ((mps as u32) << 16)
                | (8 << 28),
            ep_caps: 1 << 30, // mult = 1
            cur_qtd: 0,
            next_qtd: qtd_base,
            alt_next_qtd: 1,
            token: 0,
            buf_ptrs: [0; 5],
            _pad: [0; 4],
        };

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.head_qh.horiz_link = work_phys | 0x02;

        let spins = (timeout_ms.max(20) as usize).saturating_mul(2_500);
        let mut res = Err(UsbTransferError::Timeout);
        for _ in 0..spins {
            let last_tok = unsafe { core::ptr::read_volatile(&self.qtds[count - 1].token) };
            if (last_tok & (1 << 7)) == 0 {
                res = if (last_tok & (1 << 6)) != 0 {
                    Err(UsbTransferError::Stall)
                } else {
                    Ok(())
                };
                break;
            }
            core::hint::spin_loop();
        }

        self.head_qh.horiz_link = head_phys | 0x02;
        res
    }

    pub fn enumerate_devices(&mut self) -> Vec<UsbAttachedDevice> {
        let mut attached = Vec::new();
        for port in 0..self.port_count {
            if !self.reset_port(port) {
                continue;
            }
            let addr = self.next_addr;
            self.next_addr = self.next_addr.saturating_add(1);
            if self.control_transfer(0, 0x00, 5, addr as u16, 0, &mut []).is_err() {
                continue;
            }
            self.devices.push(EhciDeviceEntry {
                addr,
                port: port as u8,
                max_pkt0: 64,
                toggles: [false; 32],
            });
            if let Some(info) = query_device_descriptors(self, self.bus_id, addr, addr, 3, 64) {
                if let Some(d) = self.devices.iter_mut().find(|d| d.addr == addr) {
                    d.max_pkt0 = info.max_packet_size;
                }
                attached.push(info);
            }
        }
        attached
    }
}

impl UsbHostController for EhciController {
    fn bus_id(&self) -> u8 {
        self.bus_id
    }

    fn controller_name(&self) -> &'static str {
        "ehci"
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
        let is_in = (bm_request_type & 0x80) != 0;
        let mps0 = self
            .devices
            .iter()
            .find(|d| d.addr == slot_id)
            .map(|d| d.max_pkt0)
            .unwrap_or(64);

        self.setup_buf[0] = bm_request_type;
        self.setup_buf[1] = b_request;
        self.setup_buf[2..4].copy_from_slice(&w_value.to_le_bytes());
        self.setup_buf[4..6].copy_from_slice(&w_index.to_le_bytes());
        self.setup_buf[6..8].copy_from_slice(&(data.len() as u16).to_le_bytes());

        if !is_in && !data.is_empty() {
            self.bounce[..data.len()].copy_from_slice(data);
        }

        let mut idx = 0usize;
        self.qtds[idx] = EhciQtd {
            next_qtd: 1,
            alt_next_qtd: 1,
            token: (1 << 7) | (2 << 8) | (3 << 10) | (8 << 16), // Active, SETUP, CERR=3, 8B
            buf_ptrs: [self.setup_buf.physical() as u32, 0, 0, 0, 0],
            _ext: [0; 8],
        };
        idx += 1;

        if !data.is_empty() {
            let pid = if is_in { 1 } else { 0 };
            self.qtds[idx] = EhciQtd {
                next_qtd: 1,
                alt_next_qtd: 1,
                token: (1 << 7)
                    | (pid << 8)
                    | (3 << 10)
                    | ((data.len() as u32) << 16)
                    | (1 << 31),
                buf_ptrs: [self.bounce.physical() as u32, 0, 0, 0, 0],
                _ext: [0; 8],
            };
            idx += 1;
        }

        let st_pid = if data.is_empty() || !is_in { 1 } else { 0 };
        self.qtds[idx] = EhciQtd {
            next_qtd: 1,
            alt_next_qtd: 1,
            token: (1 << 7) | (st_pid << 8) | (3 << 10) | (1 << 31),
            buf_ptrs: [0; 5],
            _ext: [0; 8],
        };
        idx += 1;

        self.run_qtd_chain(slot_id, 0, mps0, idx, 100)?;
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
        let pid = if is_in { 1 } else { 0 };
        let phys = if data.is_empty() {
            0
        } else {
            self.bounce.physical() as u32
        };
        self.qtds[0] = EhciQtd {
            next_qtd: 1,
            alt_next_qtd: 1,
            token: (1 << 7)
                | (pid << 8)
                | (3 << 10)
                | ((data.len() as u32) << 16)
                | ((toggle as u32) << 31),
            buf_ptrs: [phys, 0, 0, 0, 0],
            _ext: [0; 8],
        };

        self.run_qtd_chain(slot_id, ep, 512, 1, timeout_ms)?;
        let last_tok = unsafe { core::ptr::read_volatile(&self.qtds[0].token) };
        if let Some(d) = self.devices.iter_mut().find(|d| d.addr == slot_id) {
            d.toggles[toggle_slot] = (last_tok & (1 << 31)) != 0;
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
        if self.reset_port(port) {
            Ok(())
        } else {
            Err(UsbTransferError::IoError)
        }
    }
}

pub static EHCI_INSTANCES: Mutex<Vec<Arc<Mutex<EhciController>>>> = Mutex::new(Vec::new());

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager, block_count: &mut usize) -> bool {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x20 {
        match EhciController::new(dev) {
            Ok(ehci) => {
                dev.driver_name = Some("ehci");
                let ports = ehci.port_count;
                let bus = ehci.bus_id;
                let arc = Arc::new(Mutex::new(ehci));
                crate::klog!(
                    "ehci: {} ({:04x}:{:04x}, {} {}, bus {})",
                    t(
                        "активирован контроллер USB 2.0 EHCI",
                        "registered USB 2.0 EHCI Host Controller"
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
                EHCI_INSTANCES.lock().push(arc);
                return true;
            }
            Err(e) => {
                serial_println!("ehci: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
