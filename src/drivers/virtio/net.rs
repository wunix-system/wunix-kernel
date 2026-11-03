use alloc::boxed::Box;
use core::sync::atomic::{fence, Ordering};

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::drivers::NetDevice;

// Флаги заголовка пакета VirtIO-Net
pub const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_NONE: u8 = 0;

/// Заголовок сетевого пакета VirtIO (10 байт)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
}

const RX_BUFFER_SIZE: usize = 2048;
const RX_QUEUE_BUFFERS: usize = 16;

/// Драйвер сетевого адаптера VirtIO-Net
pub struct VirtioNet {
    transport: Box<dyn VirtioTransport>,
    rx_queue: Virtqueue,
    tx_queue: Virtqueue,
    mac: [u8; 6],
    // Выделенный физический буфер под приемные пакеты RX
    rx_dma_phys: u64,
    rx_dma_virt: *mut u8,
    // Выделенный физический буфер под передачу пакетов TX
    tx_dma_phys: u64,
    tx_dma_virt: *mut u8,
}

unsafe impl Send for VirtioNet {}
unsafe impl Sync for VirtioNet {}

impl VirtioNet {
    /// Инициализирует сетевой адаптер VirtIO-Net (очереди RX/TX и считывание MAC)
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();

        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        transport.set_status(status);

        status |= VIRTIO_STATUS_DRIVER;
        transport.set_status(status);

        let _features = transport.read_device_features();
        // В базовом режиме отключаем Mergeable RX buffers и контрольную сумму,
        // используя классический 10-байтный заголовок VirtioNetHdr
        transport.write_driver_features(0);

        status |= VIRTIO_STATUS_FEATURES_OK;
        transport.set_status(status);

        // В Legacy PCI VirtIO размер очереди фиксирован хостом и считывается из регистра Queue Size.
        // Выделяем очередь точно по размеру, запрошенному хостом, чтобы не нарушать смещения колец avail/used.
        transport.select_queue(0);
        let rx_max = transport.get_queue_size();
        let rx_size = rx_max;
        let mut rx_queue = Virtqueue::new(0, rx_size)?;
        transport.setup_queue(&rx_queue);

        // Инициализируем очередь передачи (Queue 1 - TX)
        transport.select_queue(1);
        let tx_max = transport.get_queue_size();
        let tx_size = tx_max;
        let tx_queue = Virtqueue::new(1, tx_size)?;
        transport.setup_queue(&tx_queue);

        status |= VIRTIO_STATUS_DRIVER_OK;
        transport.set_status(status);

        // Читаем аппаратный MAC-адрес устройства (первые 6 байт конфигурации)
        let mut mac = [0u8; 6];
        transport.read_device_config(0, &mut mac);

        // Выделяем память под RX буферы (16 буферов по 2 КиБ = 32 КиБ = 8 страниц)
        let rx_pages = 8;
        let rx_frame = crate::memory::allocate_contiguous_frames(rx_pages)
            .ok_or(VirtioError::AllocationFailed)?;
        let rx_dma_phys = rx_frame.start_address().as_u64();
        let hhdm = crate::memory::hhdm_offset();
        let rx_dma_virt = (hhdm + rx_dma_phys) as *mut u8;

        // Выделяем память под TX буфер (1 страница = 4 КиБ)
        let tx_frame = crate::memory::allocate_contiguous_frames(1)
            .ok_or(VirtioError::AllocationFailed)?;
        let tx_dma_phys = tx_frame.start_address().as_u64();
        let tx_dma_virt = (hhdm + tx_dma_phys) as *mut u8;

        // Предварительно заполняем RX очередь буферами для приема входящих кадров
        for i in 0..RX_QUEUE_BUFFERS {
            let offset = i * RX_BUFFER_SIZE;
            let buf = [VirtioBuffer {
                phys_addr: rx_dma_phys + (offset as u64),
                len: RX_BUFFER_SIZE as u32,
                is_write: true, // Устройство записывает входящий пакет
            }];
            let _ = rx_queue.add_buffer(&buf);
        }
        transport.notify_queue(0);

        Ok(Self {
            transport,
            rx_queue,
            tx_queue,
            mac,
            rx_dma_phys,
            rx_dma_virt,
            tx_dma_phys,
            tx_dma_virt,
        })
    }
}

impl VirtioDevice for VirtioNet {
    fn name(&self) -> &str {
        "VirtIO-Net"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Net
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl NetDevice for VirtioNet {
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), VirtioError> {
        if packet.is_empty() || packet.len() > 1514 {
            return Err(VirtioError::Unsupported);
        }

        let hdr_size = core::mem::size_of::<VirtioNetHdr>();

        unsafe {
            // Заголовок пакета VirtIO-Net
            let hdr_ptr = self.tx_dma_virt as *mut VirtioNetHdr;
            core::ptr::write_volatile(hdr_ptr, VirtioNetHdr::default());

            // Копируем сам сетевой Ethernet-фрейм
            let data_ptr = self.tx_dma_virt.add(hdr_size);
            core::ptr::copy_nonoverlapping(packet.as_ptr(), data_ptr, packet.len());
        }

        fence(Ordering::SeqCst);

        let buffers = [
            VirtioBuffer {
                phys_addr: self.tx_dma_phys,
                len: hdr_size as u32,
                is_write: false,
            },
            VirtioBuffer {
                phys_addr: self.tx_dma_phys + (hdr_size as u64),
                len: packet.len() as u32,
                is_write: false,
            },
        ];

        let _head = self.tx_queue.add_buffer(&buffers)?;
        self.transport.notify_queue(1);

        // Ожидаем подтверждения отправки устройством
        let mut spins = 0u64;
        while !self.tx_queue.can_pop() {
            x86_64::instructions::nop();
            spins += 1;
            if spins > 5_000_000 {
                return Err(VirtioError::IoError);
            }
        }

        let (popped_head, _) = self.tx_queue.pop_used().ok_or(VirtioError::IoError)?;
        self.tx_queue.free_chain(popped_head);

        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, VirtioError> {
        if !self.rx_queue.can_pop() {
            return Ok(None);
        }

        let (head, bytes_written) = self.rx_queue.pop_used().ok_or(VirtioError::IoError)?;
        let hdr_size = core::mem::size_of::<VirtioNetHdr>();

        if (bytes_written as usize) <= hdr_size {
            self.rx_queue.free_chain(head);
            return Ok(None);
        }

        let packet_len = (bytes_written as usize) - hdr_size;
        let copy_len = packet_len.min(buf.len());

        // Получаем точный физический адрес буфера напрямую из дескриптора головы
        let buf_phys = self.rx_queue.desc_addr(head);
        let buf_offset = buf_phys.saturating_sub(self.rx_dma_phys) as usize;
        unsafe {
            let data_ptr = self.rx_dma_virt.add(buf_offset + hdr_size);
            core::ptr::copy_nonoverlapping(data_ptr, buf.as_mut_ptr(), copy_len);
        }

        // Возвращаем буфер обратно в очередь приема
        self.rx_queue.free_chain(head);
        let replenish = [VirtioBuffer {
            phys_addr: buf_phys,
            len: RX_BUFFER_SIZE as u32,
            is_write: true,
        }];
        let _ = self.rx_queue.add_buffer(&replenish);
        self.transport.notify_queue(0);

        Ok(Some(copy_len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_net_header_size() {
        assert_eq!(core::mem::size_of::<VirtioNetHdr>(), 10);
    }
}
