use alloc::boxed::Box;
use core::sync::atomic::{fence, Ordering};

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::drivers::{BlockDevice, BlockDeviceError};

// Типы операций VirtIO Block
pub const VIRTIO_BLK_T_IN: u32 = 0; // Чтение секторов (Read)
pub const VIRTIO_BLK_T_OUT: u32 = 1; // Запись секторов (Write)
pub const VIRTIO_BLK_T_FLUSH: u32 = 4; // Сброс кэша диска (Flush)

// Статусы завершения операции блочным устройством
pub const VIRTIO_BLK_S_OK: u8 = 0;
pub const VIRTIO_BLK_S_IOERR: u8 = 1;
pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;

pub const SECTOR_SIZE: usize = 512;

/// Заголовок запроса блочного устройства (16 байт)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioBlkReqHeader {
    pub type_: u32,
    pub reserved: u32,
    pub sector: u64,
}

/// Выделенный DMA-буфер для выполнения синхронных дисковых операций
struct DmaBuffer {
    phys_addr: u64,
    virt_addr: *mut u8,
    _page_count: usize,
}

impl DmaBuffer {
    fn new(pages: usize) -> Result<Self, VirtioError> {
        let frame = crate::memory::allocate_contiguous_frames(pages)
            .ok_or(VirtioError::AllocationFailed)?;
        let phys_addr = frame.start_address().as_u64();
        let hhdm = crate::memory::hhdm_offset();
        let virt_addr = (hhdm + phys_addr) as *mut u8;

        unsafe {
            core::ptr::write_bytes(virt_addr, 0, pages * 4096);
        }

        Ok(Self {
            phys_addr,
            virt_addr,
            _page_count: pages,
        })
    }
}

/// Драйвер блочного накопителя VirtIO-Block (/dev/vda)
pub struct VirtioBlock {
    transport: Box<dyn VirtioTransport>,
    queue: Virtqueue,
    capacity_sectors: u64,
    dma_buf: DmaBuffer,
}

unsafe impl Send for VirtioBlock {}
unsafe impl Sync for VirtioBlock {}

impl VirtioBlock {
    /// Инициализирует блочное устройство VirtIO по стандартной процедуре согласования
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        // 1. Сброс устройства
        transport.reset();

        // 2. Установка бита ACKNOWLEDGE (ОС обнаружила устройство)
        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        transport.set_status(status);

        // 3. Установка бита DRIVER (ОС умеет работать с данным устройством)
        status |= VIRTIO_STATUS_DRIVER;
        transport.set_status(status);

        // 4. Чтение и согласование поддерживаемых возможностей
        let _features = transport.read_device_features();
        // В базовом режиме не требуем специфичных расширений вроде readonly или mq
        transport.write_driver_features(0);

        // 5. Установка бита FEATURES_OK
        status |= VIRTIO_STATUS_FEATURES_OK;
        transport.set_status(status);

        // 6. Инициализация очереди запросов (Queue 0)
        transport.select_queue(0);
        let max_size = transport.get_queue_size();
        if max_size == 0 {
            return Err(VirtioError::QueueTooSmall);
        }

        // В Legacy PCI VirtIO размер очереди фиксирован хостом и считывается из регистра Queue Size.
        // Если драйвер выделит очередь меньшего размера, смещения avail и used rings не совпадут
        // со спецификацией Legacy VirtIO (где QEMU рассчитывает их по формуле от max_size),
        // что приведёт к ошибке DMA.
        let queue_size = max_size;
        let queue = Virtqueue::new(0, queue_size)?;
        transport.setup_queue(&queue);

        // 7. Установка бита DRIVER_OK (драйвер готов к работе)
        status |= VIRTIO_STATUS_DRIVER_OK;
        transport.set_status(status);

        // 8. Чтение емкости накопителя из конфигурационного пространства (offset 0..8: u64 секторов)
        let mut cap_bytes = [0u8; 8];
        transport.read_device_config(0, &mut cap_bytes);
        let capacity_sectors = u64::from_le_bytes(cap_bytes);

        // 2 страницы (8 КиБ) под DMA-буфер: 16 байт заголовок + до 15 секторов данных + 1 байт статус
        let dma_buf = DmaBuffer::new(2)?;

        Ok(Self {
            transport,
            queue,
            capacity_sectors,
            dma_buf,
        })
    }

    /// Внутреннее выполнение операции чтения/записи LBA-сектора
    fn do_io(
        &mut self,
        req_type: u32,
        start_sector: u64,
        buffer: &mut [u8],
    ) -> Result<(), VirtioError> {
        let sector_count = buffer.len() / SECTOR_SIZE;
        if sector_count == 0 {
            return Ok(());
        }

        // Заголовок запроса размещаем в начале DMA-буфера (offset 0)
        let hdr_ptr = self.dma_buf.virt_addr as *mut VirtioBlkReqHeader;
        let data_virt = unsafe { self.dma_buf.virt_addr.add(64) };
        let status_virt = unsafe { self.dma_buf.virt_addr.add(64 + buffer.len()) };

        let hdr_phys = self.dma_buf.phys_addr;
        let data_phys = self.dma_buf.phys_addr + 64;
        let status_phys = self.dma_buf.phys_addr + 64 + (buffer.len() as u64);

        unsafe {
            core::ptr::write_volatile(
                hdr_ptr,
                VirtioBlkReqHeader {
                    type_: req_type,
                    reserved: 0,
                    sector: start_sector,
                },
            );

            // При записи копируем данные в физический DMA-буфер
            if req_type == VIRTIO_BLK_T_OUT {
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), data_virt, buffer.len());
            }

            // Инициализируем байт статуса недопустимым значением перед запросом
            core::ptr::write_volatile(status_virt, 0xFF);
        }

        fence(Ordering::SeqCst);

        // Формируем цепочку из 3 дескрипторов:
        // 1. Заголовок (чтение устройством)
        // 2. Данные (запись устройством при чтении, либо чтение устройством при записи)
        // 3. Байт статуса (запись устройством)
        let is_write_for_device = req_type == VIRTIO_BLK_T_IN;

        let buffers = [
            VirtioBuffer {
                phys_addr: hdr_phys,
                len: core::mem::size_of::<VirtioBlkReqHeader>() as u32,
                is_write: false,
            },
            VirtioBuffer {
                phys_addr: data_phys,
                len: buffer.len() as u32,
                is_write: is_write_for_device,
            },
            VirtioBuffer {
                phys_addr: status_phys,
                len: 1,
                is_write: true,
            },
        ];

        let _head = self.queue.add_buffer(&buffers)?;
        self.transport.notify_queue(0);

        // Ожидаем завершения транзакции устройством
        let mut spins = 0u64;
        while !self.queue.can_pop() {
            x86_64::instructions::nop();
            spins += 1;
            if spins > 10_000_000 {
                return Err(VirtioError::IoError);
            }
        }

        let (popped_head, _) = self.queue.pop_used().ok_or(VirtioError::IoError)?;
        self.queue.free_chain(popped_head);

        fence(Ordering::SeqCst);

        let status = unsafe { core::ptr::read_volatile(status_virt) };
        if status != VIRTIO_BLK_S_OK {
            return Err(VirtioError::DeviceError(status));
        }

        // При чтении копируем результат из DMA-буфера в целевой буфер вызывающего
        if req_type == VIRTIO_BLK_T_IN {
            unsafe {
                core::ptr::copy_nonoverlapping(data_virt, buffer.as_mut_ptr(), buffer.len());
            }
        }

        Ok(())
    }
}

impl VirtioDevice for VirtioBlock {
    fn name(&self) -> &str {
        "VirtIO-Block (/dev/vda)"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Block
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl BlockDevice for VirtioBlock {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        if buf.is_empty() {
            return Ok(());
        }
        if buf.len() % SECTOR_SIZE != 0 {
            return Err(BlockDeviceError::Unsupported);
        }

        let sector_count = (buf.len() / SECTOR_SIZE) as u64;
        if start_sector + sector_count > self.capacity_sectors {
            return Err(BlockDeviceError::OutOfBounds);
        }

        // Выполняем чтение порциями не более 14 секторов (7 КиБ) за раз, чтобы уместиться в DMA буфер
        let max_chunk_sectors = 14;
        let mut current_sector = start_sector;
        let mut offset = 0;

        while offset < buf.len() {
            let remaining_sectors = (buf.len() - offset) / SECTOR_SIZE;
            let chunk_sectors = remaining_sectors.min(max_chunk_sectors);
            let chunk_bytes = chunk_sectors * SECTOR_SIZE;

            self.do_io(
                VIRTIO_BLK_T_IN,
                current_sector,
                &mut buf[offset..offset + chunk_bytes],
            ).map_err(BlockDeviceError::from)?;

            current_sector += chunk_sectors as u64;
            offset += chunk_bytes;
        }

        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        if buf.is_empty() {
            return Ok(());
        }
        if buf.len() % SECTOR_SIZE != 0 {
            return Err(BlockDeviceError::Unsupported);
        }

        let sector_count = (buf.len() / SECTOR_SIZE) as u64;
        if start_sector + sector_count > self.capacity_sectors {
            return Err(BlockDeviceError::OutOfBounds);
        }

        let max_chunk_sectors = 14;
        let mut current_sector = start_sector;
        let mut offset = 0;

        // Для записи копируем порции во временный срез и шлем устройству
        while offset < buf.len() {
            let remaining_sectors = (buf.len() - offset) / SECTOR_SIZE;
            let chunk_sectors = remaining_sectors.min(max_chunk_sectors);
            let chunk_bytes = chunk_sectors * SECTOR_SIZE;

            // do_io для VIRTIO_BLK_T_OUT читает из buf, создаем мутабельную обертку над порцией
            let mut chunk = alloc::vec![0u8; chunk_bytes];
            chunk.copy_from_slice(&buf[offset..offset + chunk_bytes]);

            self.do_io(VIRTIO_BLK_T_OUT, current_sector, &mut chunk).map_err(BlockDeviceError::from)?;

            current_sector += chunk_sectors as u64;
            offset += chunk_bytes;
        }

        Ok(())
    }

    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        self.capacity_sectors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_blk_header_size_and_alignment() {
        assert_eq!(core::mem::size_of::<VirtioBlkReqHeader>(), 16);
    }
}
