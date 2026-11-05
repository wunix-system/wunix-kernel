use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::{fence, Ordering};

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::drivers::{InputDevice, InputEvent};

// Типы событий стандарта Linux Input Event
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

/// Событие ввода VirtIO (псевдоним нейтрального типа InputEvent)
pub type VirtioInputEvent = InputEvent;

const EVENT_BUFFER_COUNT: usize = 32;

/// Драйвер устройств ввода VirtIO-Input (планшет / мышь / клавиатура)
pub struct VirtioInput {
    transport: Box<dyn VirtioTransport>,
    event_queue: Virtqueue,
    _status_queue: Virtqueue,
    name: String,
    dma_phys: u64,
    dma_virt: *mut VirtioInputEvent,
    // Текущие координаты указателя (абсолютные координаты планшета X/Y 0..32767)
    x: u32,
    y: u32,
    buttons: u32,
}

unsafe impl Send for VirtioInput {}
unsafe impl Sync for VirtioInput {}

impl VirtioInput {
    /// Инициализирует устройство ввода VirtIO (Queue 0 - события ввода, Queue 1 - статус)
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();

        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        transport.set_status(status);

        status |= VIRTIO_STATUS_DRIVER;
        transport.set_status(status);

        let _features = transport.read_device_features();
        transport.write_driver_features(0);

        status |= VIRTIO_STATUS_FEATURES_OK;
        transport.set_status(status);

        // Инициализируем очередь событий ввода (Queue 0)
        transport.select_queue(0);
        let ev_max = transport.get_queue_size();
        let ev_size = ev_max;
        let mut event_queue = Virtqueue::new(0, ev_size)?;
        transport.setup_queue(&event_queue);

        // Инициализируем очередь статуса (Queue 1)
        transport.select_queue(1);
        let st_max = transport.get_queue_size();
        let st_size = st_max;
        let status_queue = Virtqueue::new(1, st_size)?;
        transport.setup_queue(&status_queue);

        status |= VIRTIO_STATUS_DRIVER_OK;
        transport.set_status(status);

        // Читаем имя устройства из конфигурации VirtIO-Input:
        // select = 1 (VIRTIO_INPUT_CFG_ID_NAME)
        transport.write_device_config(0, &[1u8]); // select
        transport.write_device_config(1, &[0u8]); // subsel

        let mut name_size = [0u8; 1];
        transport.read_device_config(2, &mut name_size);
        let size = (name_size[0] as usize).min(64);

        let mut name_buf = [0u8; 64];
        if size > 0 {
            transport.read_device_config(8, &mut name_buf[..size]);
        }
        let name_str = core::str::from_utf8(&name_buf[..size]).unwrap_or("VirtIO Input");
        let name = String::from(name_str);

        // Выделяем 1 страницу под пул буферов событий ввода (32 события по 8 байт = 256 байт)
        let frame = crate::memory::allocate_contiguous_frames(1)
            .ok_or(VirtioError::AllocationFailed)?;
        let dma_phys = frame.start_address().as_u64();
        let hhdm = crate::memory::hhdm_offset();
        let dma_virt = (hhdm + dma_phys) as *mut VirtioInputEvent;

        // Предварительно заполняем очередь буферами под события от устройства
        let ev_size_bytes = core::mem::size_of::<VirtioInputEvent>() as u32;
        for i in 0..EVENT_BUFFER_COUNT {
            let offset = (i as u64) * (ev_size_bytes as u64);
            let buf = [VirtioBuffer {
                phys_addr: dma_phys + offset,
                len: ev_size_bytes,
                is_write: true, // Устройство пишет данные событий
            }];
            let _ = event_queue.add_buffer(&buf);
        }
        transport.notify_queue(0);

        Ok(Self {
            transport,
            event_queue,
            _status_queue: status_queue,
            name,
            dma_phys,
            dma_virt,
            x: 0,
            y: 0,
            buttons: 0,
        })
    }

    pub fn x(&self) -> u32 {
        self.x
    }

    pub fn y(&self) -> u32 {
        self.y
    }

    pub fn buttons(&self) -> u32 {
        self.buttons
    }
}

impl VirtioDevice for VirtioInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Input
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl InputDevice for VirtioInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn poll_event(&mut self) -> Option<VirtioInputEvent> {
        if !self.event_queue.can_pop() {
            return None;
        }

        let (head, bytes_written) = self.event_queue.pop_used()?;
        let ev_size_bytes = core::mem::size_of::<VirtioInputEvent>();

        if (bytes_written as usize) < ev_size_bytes {
            self.event_queue.free_chain(head);
            return None;
        }

        let slot = (head as usize) % EVENT_BUFFER_COUNT;
        fence(Ordering::SeqCst);
        let event = unsafe { core::ptr::read_volatile(self.dma_virt.add(slot)) };

        // Обновляем внутреннее состояние координат и кнопок
        match event.type_ {
            EV_ABS => match event.code {
                ABS_X => self.x = event.value,
                ABS_Y => self.y = event.value,
                _ => {}
            },
            EV_KEY => match event.code {
                BTN_LEFT => {
                    if event.value != 0 {
                        self.buttons |= 1;
                    } else {
                        self.buttons &= !1;
                    }
                }
                BTN_RIGHT => {
                    if event.value != 0 {
                        self.buttons |= 2;
                    } else {
                        self.buttons &= !2;
                    }
                }
                BTN_MIDDLE => {
                    if event.value != 0 {
                        self.buttons |= 4;
                    } else {
                        self.buttons &= !4;
                    }
                }
                _ => {}
            },
            _ => {}
        }

        // Возвращаем буфер обратно в очередь для следующих событий
        self.event_queue.free_chain(head);
        let replenish = [VirtioBuffer {
            phys_addr: self.dma_phys + ((slot * ev_size_bytes) as u64),
            len: ev_size_bytes as u32,
            is_write: true,
        }];
        let _ = self.event_queue.add_buffer(&replenish);
        self.transport.notify_queue(0);

        Some(event)
    }

    fn current_coordinates(&self) -> (u32, u32) {
        (self.x, self.y)
    }

    fn button_mask(&self) -> u32 {
        self.buttons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_input_event_size() {
        assert_eq!(core::mem::size_of::<VirtioInputEvent>(), 8);
    }
}
