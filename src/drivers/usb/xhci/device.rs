use crate::drivers::dma::Dma;
use super::context::{DeviceContext, InputContext};
use super::regs::CC_SUCCESS;
use super::ring::{EventRing, TrbRing};
use super::trb::{
    Trb, TRB_IOC, TRB_TYPE_DATA, TRB_TYPE_SETUP, TRB_TYPE_STATUS, TRB_TYPE_TRANSFER_EVENT,
};

/// Подключенное USB устройство на слоте xHCI
pub struct XhciDeviceSlot {
    pub slot_id: u8,
    pub port: u8,
    pub speed: u8,
    pub input_ctx: Dma<InputContext>,
    pub device_ctx: Dma<DeviceContext>,
    pub ep0_ring: TrbRing,
    pub bulk_in_ring: Option<TrbRing>,
    pub bulk_out_ring: Option<TrbRing>,
    pub intr_in_ring: Option<TrbRing>,
    pub ep_bulk_in: u8,
    pub ep_bulk_out: u8,
    pub ep_intr_in: u8,
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
            bulk_in_ring: None,
            bulk_out_ring: None,
            intr_in_ring: None,
            ep_bulk_in: 0,
            ep_bulk_out: 0,
            ep_intr_in: 0,
            max_packet_size,
        })
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
    ) -> Result<usize, &'static str> {
        let is_in = (bm_request_type & 0x80) != 0;
        let data_len = data.len() as u16;

        // 1. Setup Stage TRB
        let setup_data = (bm_request_type as u64)
            | ((b_request as u64) << 8)
            | ((w_value as u64) << 16)
            | ((w_index as u64) << 32)
            | ((data_len as u64) << 48);

        let trt = if data_len == 0 {
            0 // No data stage
        } else if is_in {
            3 // IN data stage
        } else {
            2 // OUT data stage
        };

        let setup_control = (TRB_TYPE_SETUP << 10) | (trt << 16) | (1 << 6); // IDT = 1
        self.ep0_ring.enqueue(Trb::new(setup_data, 8, setup_control));

        // 2. Data Stage TRB (если есть передача данных)
        if data_len > 0 {
            if !is_in {
                dma_bounce[..data.len()].copy_from_slice(data);
            }
            let data_dir = if is_in { 1 << 16 } else { 0 };
            let data_control = (TRB_TYPE_DATA << 10) | data_dir;
            self.ep0_ring.enqueue(Trb::new(
                dma_bounce.physical() as u64,
                data_len as u32,
                data_control,
            ));
        }

        // 3. Status Stage TRB
        // Направление статуса противоположно этапу данных (или IN при отсутствии данных)
        let status_dir = if data_len == 0 || !is_in { 1 << 16 } else { 0 };
        let status_control = (TRB_TYPE_STATUS << 10) | status_dir | TRB_IOC;
        let status_trb_phys = self.ep0_ring.enqueue(Trb::new(0, 0, status_control));

        // Звоним в дверной звонок слота (Target = 1 для Control EP0)
        unsafe {
            let db_ptr = db_base.add(self.slot_id as usize * 4) as *mut u32;
            core::ptr::write_volatile(db_ptr, 1);
        }

        // Ожидаем завершения передачи по Event Ring
        for _ in 0..150_000 {
            if let Some(ev) = events.poll_event() {
                // Обновляем ERDP в Interrupter 0
                unsafe {
                    let erdp_ptr = rt_base.add(super::regs::RT_ERDP) as *mut u64;
                    core::ptr::write_volatile(erdp_ptr, events.current_dequeue_phys() | (1 << 3));
                }

                if ev.trb_type() == TRB_TYPE_TRANSFER_EVENT {
                    let cc = (ev.status >> 24) & 0xFF;
                    if cc == CC_SUCCESS || cc == super::regs::CC_SHORT_PACKET {
                        if is_in && data_len > 0 {
                            data.copy_from_slice(&dma_bounce[..data.len()]);
                        }
                        return Ok(data.len());
                    } else if ev.data_ptr == status_trb_phys {
                        return Err("xhci: control transfer failed");
                    }
                }
            }
            core::hint::spin_loop();
        }

        Err("xhci: control transfer timeout")
    }

    /// Получение стандартного дескриптора USB
    pub fn get_descriptor(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        desc_type: u8,
        desc_index: u8,
        buf: &mut [u8],
        dma_bounce: &mut Dma<[u8; 4096]>,
    ) -> Result<usize, &'static str> {
        let w_val = ((desc_type as u16) << 8) | (desc_index as u16);
        self.control_transfer(
            events, db_base, rt_base,
            0x80, // Device-to-Host, Standard, Device
            6,    // GET_DESCRIPTOR
            w_val,
            0,
            buf,
            dma_bounce,
        )
    }

    /// Установка активной конфигурации устройства (SET_CONFIGURATION)
    pub fn set_configuration(
        &mut self,
        events: &mut EventRing,
        db_base: *mut u8,
        rt_base: *mut u8,
        config_val: u8,
        dma_bounce: &mut Dma<[u8; 4096]>,
    ) -> Result<(), &'static str> {
        self.control_transfer(
            events, db_base, rt_base,
            0x00, // Host-to-Device, Standard, Device
            9,    // SET_CONFIGURATION
            config_val as u16,
            0,
            &mut [],
            dma_bounce,
        )?;
        Ok(())
    }
}
