use core::sync::atomic::{fence, Ordering};
use x86_64::structures::paging::PhysFrame;

use super::VirtioError;

// Флаги дескрипторов Virtqueue (VirtqDesc flags)
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;

/// Элемент таблицы дескрипторов кольца Virtqueue (16 байт)
#[repr(C, align(16))]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtqDesc {
    /// Физический адрес буфера в памяти гостя
    pub addr: u64,
    /// Длина буфера в байтах
    pub len: u32,
    /// Флаги (NEXT, WRITE, INDIRECT)
    pub flags: u16,
    /// Индекс следующего дескриптора при активном флаге NEXT
    pub next: u16,
}

/// Элемент кольца завершенных запросов (Used Ring Element)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtqUsedElem {
    /// Индекс начального дескриптора цепочки
    pub id: u32,
    /// Количество байт, записанных устройством в буфер
    pub len: u32,
}

/// Буфер для добавления в очередь Virtqueue
#[derive(Debug, Clone, Copy)]
pub struct VirtioBuffer {
    pub phys_addr: u64,
    pub len: u32,
    pub is_write: bool,
}

/// Реализация кольцевой очереди VirtIO (Split Virtqueue)
pub struct Virtqueue {
    /// Индекс очереди в устройстве (0, 1, 2...)
    queue_index: u16,
    /// Количество дескрипторов в очереди (степень двойки: 16, 32, 64, 128...)
    queue_size: u16,
    /// Базовый физический фрейм выделенной памяти
    phys_frame: PhysFrame,
    /// Количество выделенных страниц памяти
    _page_count: usize,
    /// Указатель на таблицу дескрипторов (в окне HHDM)
    desc_table: *mut VirtqDesc,
    /// Указатель на кольцо доступных буферов (Available Ring)
    avail_ring: *mut u16,
    /// Указатель на кольцо использованных буферов (Used Ring)
    used_ring: *mut u8,
    /// Голова списка свободных дескрипторов
    free_head: u16,
    /// Количество оставшихся свободных дескрипторов
    num_free: u16,
    /// Последний обработанный драйвером индекс в Used Ring
    last_used_idx: u16,
}

unsafe impl Send for Virtqueue {}
unsafe impl Sync for Virtqueue {}

impl Virtqueue {
    /// Создает и инициализирует новую кольцевую очередь Virtqueue.
    /// Выделяет непрерывные физические страницы под структуры Desc, Avail и Used.
    pub fn new(queue_index: u16, queue_size: u16) -> Result<Self, VirtioError> {
        if queue_size == 0 || (queue_size & (queue_size - 1)) != 0 {
            return Err(VirtioError::QueueTooSmall);
        }

        // Вычисляем размеры секций по спецификации Split Virtqueue
        let size = queue_size as usize;
        let desc_size = 16 * size;
        let avail_size = 6 + 2 * size;
        // В Legacy VirtIO кольцо Used Ring обязано быть выровнено на границу 4096 байт
        let used_offset = (desc_size + avail_size + 4095) & !4095;
        let used_size = 6 + 8 * size;
        let total_bytes = used_offset + used_size;
        let page_count = (total_bytes + 4095) / 4096;

        let phys_frame = crate::memory::allocate_contiguous_frames(page_count)
            .ok_or(VirtioError::AllocationFailed)?;

        let hhdm = crate::memory::hhdm_offset();
        let virt_base = (hhdm + phys_frame.start_address().as_u64()) as *mut u8;

        // Обнуляем всю выделенную память под очередь
        unsafe {
            core::ptr::write_bytes(virt_base, 0, page_count * 4096);
        }

        let desc_table = virt_base as *mut VirtqDesc;
        let avail_ring = unsafe { virt_base.add(desc_size) } as *mut u16;
        let used_ring = unsafe { virt_base.add(used_offset) };

        // Инициализируем связанный список свободных дескрипторов
        unsafe {
            for i in 0..queue_size {
                let desc = &mut *desc_table.add(i as usize);
                desc.next = (i + 1) % queue_size;
            }
        }

        Ok(Self {
            queue_index,
            queue_size,
            phys_frame,
            _page_count: page_count,
            desc_table,
            avail_ring,
            used_ring,
            free_head: 0,
            num_free: queue_size,
            last_used_idx: 0,
        })
    }

    #[inline]
    pub fn queue_index(&self) -> u16 {
        self.queue_index
    }

    #[inline]
    pub fn queue_size(&self) -> u16 {
        self.queue_size
    }

    /// Физический адрес начала очереди (используется для Legacy PCI PFN = addr / 4096)
    #[inline]
    pub fn phys_addr(&self) -> u64 {
        self.phys_frame.start_address().as_u64()
    }

    /// Физический адрес таблицы дескрипторов
    #[inline]
    pub fn desc_phys(&self) -> u64 {
        self.phys_addr()
    }

    /// Физический адрес кольца Available Ring
    #[inline]
    pub fn avail_phys(&self) -> u64 {
        self.phys_addr() + (16 * self.queue_size as u64)
    }

    /// Физический адрес кольца Used Ring (выровнен по 4096 байт)
    #[inline]
    pub fn used_phys(&self) -> u64 {
        let desc_size = 16 * self.queue_size as u64;
        let avail_size = 6 + 2 * self.queue_size as u64;
        let used_offset = (desc_size + avail_size + 4095) & !4095;
        self.phys_addr() + used_offset
    }

    /// Добавляет цепочку буферов в очередь и возвращает индекс дескриптора головы цепочки.
    pub fn add_buffer(&mut self, buffers: &[VirtioBuffer]) -> Result<u16, VirtioError> {
        let count = buffers.len() as u16;
        if count == 0 || self.num_free < count {
            return Err(VirtioError::QueueFull);
        }

        let head = self.free_head;
        let mut curr = head;

        for (i, buf) in buffers.iter().enumerate() {
            let is_last = i == buffers.len() - 1;
            let next_desc = unsafe { (*self.desc_table.add(curr as usize)).next };

            let desc = unsafe { &mut *self.desc_table.add(curr as usize) };
            desc.addr = buf.phys_addr;
            desc.len = buf.len;
            desc.flags = 0;

            if buf.is_write {
                desc.flags |= VIRTQ_DESC_F_WRITE;
            }

            if !is_last {
                desc.flags |= VIRTQ_DESC_F_NEXT;
                desc.next = next_desc;
                curr = next_desc;
            }
        }

        // Обновляем голову списка свободных дескрипторов
        self.free_head = unsafe { (*self.desc_table.add(curr as usize)).next };
        self.num_free -= count;

        // Помещаем индекс головы в кольцо Available Ring:
        // Структура avail_ring:
        // [0] = flags (u16)
        // [1] = idx (u16)
        // [2 + (idx % queue_size)] = head (u16)
        unsafe {
            let avail_idx_ptr = self.avail_ring.add(1);
            let current_idx = core::ptr::read_volatile(avail_idx_ptr);
            let ring_slot = self.avail_ring.add(2 + ((current_idx as usize) % (self.queue_size as usize)));

            core::ptr::write_volatile(ring_slot, head);

            // Барьер памяти перед обновлением индекса доступности для DMA
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(avail_idx_ptr, current_idx.wrapping_add(1));
        }

        Ok(head)
    }

    /// Проверяет, завершило ли устройство обработку хотя бы одного запроса.
    pub fn can_pop(&self) -> bool {
        let used_idx_ptr = unsafe { (self.used_ring as *const u16).add(1) };
        let current_used_idx = unsafe { core::ptr::read_volatile(used_idx_ptr) };
        self.last_used_idx != current_used_idx
    }

    /// Извлекает завершенный запрос из Used Ring.
    /// Возвращает `Some((head_index, bytes_written))`.
    pub fn pop_used(&mut self) -> Option<(u16, u32)> {
        if !self.can_pop() {
            return None;
        }

        let slot = (self.last_used_idx as usize) % (self.queue_size as usize);
        // Структура used_ring:
        // [0..2] = flags (u16)
        // [2..4] = idx (u16)
        // [4..]  = массив VirtqUsedElem (по 8 байт каждый)
        let elem_ptr = unsafe {
            (self.used_ring.add(4) as *const VirtqUsedElem).add(slot)
        };

        fence(Ordering::SeqCst);
        let elem = unsafe { core::ptr::read_volatile(elem_ptr) };
        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        Some((elem.id as u16, elem.len))
    }

    /// Возвращает физический адрес буфера по индексу дескриптора
    #[inline]
    pub fn desc_addr(&self, desc_idx: u16) -> u64 {
        unsafe { (*self.desc_table.add(desc_idx as usize)).addr }
    }

    /// Освобождает цепочку дескрипторов, возвращая их в список свободных.
    pub fn free_chain(&mut self, head: u16) {
        let mut curr = head;
        let mut count = 0;

        loop {
            count += 1;
            let desc = unsafe { &*self.desc_table.add(curr as usize) };
            if (desc.flags & VIRTQ_DESC_F_NEXT) != 0 {
                curr = desc.next;
            } else {
                break;
            }
        }

        unsafe {
            (*self.desc_table.add(curr as usize)).next = self.free_head;
        }
        self.free_head = head;
        self.num_free += count;
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn test_virtqueue_alignment_math() {
        let queue_size = 32usize;
        let desc_size = 16 * queue_size;
        let avail_size = 6 + 2 * queue_size;
        let used_offset = (desc_size + avail_size + 4095) & !4095;
        let used_size = 6 + 8 * queue_size;
        let total = used_offset + used_size;

        assert_eq!(desc_size, 512);
        assert_eq!(avail_size, 70);
        assert_eq!(used_offset, 4096);
        assert_eq!(used_size, 262);
        assert_eq!(total, 4358);
        assert_eq!((total + 4095) / 4096, 2);
    }
}
