use alloc::vec::Vec;

use super::context::{DeviceContext, InputContext};
use super::regs::{CC_SHORT_PACKET, CC_STALL, CC_SUCCESS};
use super::ring::{EventRing, TrbRing};
use super::trb::{
    Trb, TRB_IOC, TRB_TYPE_DATA, TRB_TYPE_NORMAL, TRB_TYPE_SETUP, TRB_TYPE_STATUS,
    TRB_TYPE_TRANSFER_EVENT,
};
use crate::drivers::dma::Dma;
use crate::drivers::usb::host::{UsbEndpointMeta, UsbTransferError};

const TRB_ISP: u32 = 1 << 2;

pub struct EndpointRingEntry {
    pub dci: u8,
    pub ep_addr: u8,
    pub ring: TrbRing,
    pub armed_intr_trb: Option<u64>,
    pub armed_len: usize,
    pub intr_buf: Dma<[u8; 64]>,
}

/// Переводит стандартный bEndpointAddress (например 0x81 или 0x02) в xHCI Device Context Index (2..=31).
pub fn endpoint_dci(ep_addr: u8) -> Option<u8> {
    let ep_num = ep_addr & 0x0F;
    if ep_num == 0 || ep_num > 15 {
        return None;
    }
    let is_in = (ep_addr & 0x80) != 0;
    Some(if is_in { ep_num * 2 + 1 } else { ep_num * 2 })
}

fn encode_xhci_interval(speed: u8, attr: u8, raw_interval: u8) -> u8 {
    let ep_transfer_type = attr & 0x03;
    if ep_transfer_type == 2 || ep_transfer_type == 0 {
        return 0;
    }
    if speed >= 3 {
        // Для HS/SS в дескрипторе уже задана степень двойки (1..16) -> в xHCI хранится (N - 1).
        raw_interval.saturating_sub(1).min(15)
    } else {
        // Для LS/FS bInterval в миллисекундах (1..255 мс) переводим в 125-мкс микрокадры: ilog2(ms * 8).
        let frames = (raw_interval.max(1) as u32) * 8;
        (31 - frames.leading_zeros()).clamp(3, 11) as u8
    }
}

/// Подключенное USB устройство на слоте xHCI
pub struct XhciDeviceSlot {
    pub slot_id: u8,
    pub port: u8,
    pub speed: u8,
    pub input_ctx: Dma<InputContext>,
    pub device_ctx: Dma<DeviceContext>,
    pub ep0_ring: TrbRing,
    pub ep_rings: Vec<EndpointRingEntry>,
    pub max_packet_size: u16,
}

impl XhciDeviceSlot {
    pub fn new(slot_id: u8, port: u8, speed: u8) -> Result<Self, &'static str> {
        let ep0_ring = TrbRing::new()?;
        let input_ctx = Dma::zeroed()?;
        let device_ctx = Dma::zeroed()?;

        let max_packet_size = match speed {
            4 => 512, // SuperSpeed USB 3.0
            3 => 64,  // HighSpeed USB 2.0
            1 => 64,  // FullSpeed USB 1.1
            _ => 8,   // LowSpeed
        };

        Ok(Self {
            slot_id,
            port,
            speed,
            input_ctx,
            device_ctx,
            ep0_ring,
            ep_rings: Vec::new(),
            max_packet_size,
        })
    }

    pub fn prepare_configure_endpoints(
        &mut self,
        endpoints: &[UsbEndpointMeta],
    ) -> Result<u8, &'static str> {
        self.ep_rings.clear();
        self.input_ctx.control.drop_flags = 0;
        self.input_ctx.control.add_flags = 1 << 0; // Slot Context обязателен для CONFIG_EP
        let mut max_dci: u8 = 1;

        for ep in endpoints {
            let Some(dci) = endpoint_dci(ep.addr) else {
                continue;
            };
            let is_in = (ep.addr & 0x80) != 0;
            let ep_type = match (ep.attr & 0x03, is_in) {
                (2, true) => 6,  // Bulk IN
                (2, false) => 2, // Bulk OUT
                (3, true) => 7,  // Interrupt IN
                (3, false) => 3, // Interrupt OUT
                _ => continue,
            };

            let ring = TrbRing::new()?;
            let intr_buf = Dma::zeroed()?;
            let mps = if ep.max_packet_size == 0 {
                64
            } else {
                ep.max_packet_size
            };
            let interval = encode_xhci_interval(self.speed, ep.attr, ep.interval);

            self.input_ctx.endpoints[(dci - 1) as usize].init(
                ep_type,
                mps,
                ring.physical(),
                interval,
            );
            self.input_ctx.control.add_flags |= 1u32 << (dci as u32);
            max_dci = max_dci.max(dci);

            self.ep_rings.push(EndpointRingEntry {
                dci,
                ep_addr: ep.addr,
                ring,
                armed_intr_trb: None,
                armed_len: 0,
                intr_buf,
            });
        }

        self.input_ctx.slot.init(self.port, self.speed, max_dci);
        Ok(max_dci)
    }

    /// Выполняет USB Control Transfer через нулевую конечную точку (EP0)
    pub fn control_transfer(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        data: &mut [u8],
        dma_bounce: &mut Dma<[u8; 4096]>,
    ) -> Result<usize, UsbTransferError> {
        let is_in = (bm_request_type & 0x80) != 0;
        if data.len() > dma_bounce.len() {
            return Err(UsbTransferError::IoError);
        }
        let data_len = data.len() as u16;

        let setup_data = (bm_request_type as u64)
            | ((b_request as u64) << 8)
            | ((w_value as u64) << 16)
            | ((w_index as u64) << 32)
            | ((data_len as u64) << 48);

        let trt = if data_len == 0 {
            0
        } else if is_in {
            3
        } else {
            2
        };

        let setup_control = (TRB_TYPE_SETUP << 10) | (trt << 16) | (1 << 6); // IDT = 1
        self.ep0_ring.enqueue(Trb::new(setup_data, 8, setup_control));

        let mut data_trb_phys = 0u64;
        if data_len > 0 {
            if !is_in {
                dma_bounce[..data.len()].copy_from_slice(data);
            }
            let data_dir = if is_in { (1 << 16) | TRB_ISP } else { 0 };
            let data_control = (TRB_TYPE_DATA << 10) | data_dir;
            data_trb_phys = self.ep0_ring.enqueue(Trb::new(
                dma_bounce.physical() as u64,
                data_len as u32,
                data_control,
            ));
        }

        let status_dir = if data_len == 0 || !is_in { 1 << 16 } else { 0 };
        let status_control = (TRB_TYPE_STATUS << 10) | status_dir | TRB_IOC;
        let status_trb_phys = self.ep0_ring.enqueue(Trb::new(0, 0, status_control));

        unsafe {
            let db_ptr = db_base.add(self.slot_id as usize * 4) as *mut u32;
            core::ptr::write_volatile(db_ptr, 1);
        }

        let mut actual_len = data.len();
        for _ in 0..250_000 {
            if let Some(ev) = events.poll_event() {
                super::ring::advance_erdp(rt_base, events.current_dequeue_phys());

                if ev.trb_type() == TRB_TYPE_TRANSFER_EVENT {
                    let cc = (ev.status >> 24) & 0xFF;
                    if data_trb_phys != 0
                        && ev.data_ptr == data_trb_phys
                        && cc == CC_SHORT_PACKET
                    {
                        let residue = (ev.status & 0x00FF_FFFF) as usize;
                        actual_len = data.len().saturating_sub(residue);
                        continue;
                    }
                    if ev.data_ptr == status_trb_phys {
                        if cc == CC_SUCCESS || cc == CC_SHORT_PACKET {
                            if is_in && actual_len > 0 {
                                data[..actual_len].copy_from_slice(&dma_bounce[..actual_len]);
                            }
                            return Ok(actual_len);
                        } else if cc == CC_STALL {
                            return Err(UsbTransferError::Stall);
                        } else {
                            return Err(UsbTransferError::IoError);
                        }
                    }
                }
            }
            core::hint::spin_loop();
        }

        Err(UsbTransferError::Timeout)
    }

    pub fn bulk_transfer(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        ep_addr: u8,
        data: &mut [u8],
        dma_bounce: &mut Dma<[u8; 4096]>,
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        let slot_id = self.slot_id;
        let entry = self
            .ep_rings
            .iter_mut()
            .find(|e| e.ep_addr == ep_addr)
            .ok_or(UsbTransferError::InvalidEndpoint)?;

        let is_in = (ep_addr & 0x80) != 0;
        let mut total = 0usize;
        let spins = ((timeout_ms.max(50)) as usize).saturating_mul(2_000);

        for chunk in data.chunks_mut(dma_bounce.len()) {
            let chunk_len = chunk.len();
            if !is_in {
                dma_bounce[..chunk_len].copy_from_slice(chunk);
            }
            let flags = if is_in { TRB_ISP } else { 0 };
            let ctrl = (TRB_TYPE_NORMAL << 10) | TRB_IOC | flags;
            let trb_phys = entry.ring.enqueue(Trb::new(
                dma_bounce.physical() as u64,
                chunk_len as u32,
                ctrl,
            ));

            unsafe {
                let db_ptr = db_base.add(slot_id as usize * 4) as *mut u32;
                core::ptr::write_volatile(db_ptr, entry.dci as u32);
            }

            let mut completed = None;
            for _ in 0..spins {
                if let Some(ev) = events.poll_event() {
                    super::ring::advance_erdp(rt_base, events.current_dequeue_phys());
                    if ev.trb_type() == TRB_TYPE_TRANSFER_EVENT && ev.data_ptr == trb_phys {
                        completed = Some(ev);
                        break;
                    }
                }
                core::hint::spin_loop();
            }

            let ev = completed.ok_or(UsbTransferError::Timeout)?;
            let cc = (ev.status >> 24) & 0xFF;
            let residue = (ev.status & 0x00FF_FFFF) as usize;
            if cc == CC_SUCCESS || cc == CC_SHORT_PACKET {
                let got = chunk_len.saturating_sub(residue);
                if is_in && got > 0 {
                    chunk[..got].copy_from_slice(&dma_bounce[..got]);
                }
                total += got;
                if got < chunk_len {
                    break;
                }
            } else if cc == CC_STALL {
                return Err(UsbTransferError::Stall);
            } else {
                return Err(UsbTransferError::IoError);
            }
        }

        Ok(total)
    }

    /// Неразрушающий Interrupt IN polling: держит ровно один активный TRB в кольце,
    /// чтобы редкий опрос HID не переполнял Transfer Ring.
    pub fn interrupt_transfer(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError> {
        let slot_id = self.slot_id;
        let entry = self
            .ep_rings
            .iter_mut()
            .find(|e| e.ep_addr == ep_addr)
            .ok_or(UsbTransferError::InvalidEndpoint)?;

        let want = data.len().min(64);
        if want == 0 {
            return Ok(0);
        }

        if entry.armed_intr_trb.is_none() {
            let ctrl = (TRB_TYPE_NORMAL << 10) | TRB_IOC | TRB_ISP;
            let trb_phys = entry.ring.enqueue(Trb::new(
                entry.intr_buf.physical() as u64,
                want as u32,
                ctrl,
            ));
            entry.armed_intr_trb = Some(trb_phys);
            entry.armed_len = want;

            unsafe {
                let db_ptr = db_base.add(slot_id as usize * 4) as *mut u32;
                core::ptr::write_volatile(db_ptr, entry.dci as u32);
            }
        }

        let target_trb = entry.armed_intr_trb.unwrap();
        let armed_len = entry.armed_len;
        let max_checks = if timeout_ms == 0 {
            1
        } else {
            (timeout_ms as usize).saturating_mul(1_000)
        };

        for _ in 0..max_checks {
            if let Some(ev) = events.poll_event() {
                super::ring::advance_erdp(rt_base, events.current_dequeue_phys());
                if ev.trb_type() == TRB_TYPE_TRANSFER_EVENT && ev.data_ptr == target_trb {
                    entry.armed_intr_trb = None;
                    let cc = (ev.status >> 24) & 0xFF;
                    let residue = (ev.status & 0x00FF_FFFF) as usize;
                    if cc == CC_SUCCESS || cc == CC_SHORT_PACKET {
                        let got = armed_len.saturating_sub(residue).min(data.len());
                        data[..got].copy_from_slice(&entry.intr_buf[..got]);
                        return Ok(got);
                    } else if cc == CC_STALL {
                        return Err(UsbTransferError::Stall);
                    } else {
                        return Err(UsbTransferError::IoError);
                    }
                }
            }
            if timeout_ms > 0 {
                core::hint::spin_loop();
            }
        }

        Err(UsbTransferError::Timeout)
    }

    pub fn get_descriptor(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        desc_type: u8,
        desc_index: u8,
        buf: &mut [u8],
        dma_bounce: &mut Dma<[u8; 4096]>,
    ) -> Result<usize, UsbTransferError> {
        let w_val = ((desc_type as u16) << 8) | (desc_index as u16);
        self.control_transfer(
            events, db_base, rt_base, 0x80, 6, w_val, 0, buf, dma_bounce,
        )
    }

    pub fn set_configuration(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        config_val: u8,
        dma_bounce: &mut Dma<[u8; 4096]>,
    ) -> Result<(), UsbTransferError> {
        self.control_transfer(
            events,
            db_base,
            rt_base,
            0x00,
            9,
            config_val as u16,
            0,
            &mut [],
            dma_bounce,
        )?;
        Ok(())
    }
}
