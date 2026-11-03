use alloc::vec::Vec;
use limine::memmap::{Entry, MEMMAP_USABLE};
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

pub struct LimineFrameAllocator {
    entries: &'static [&'static Entry],
    current_entry_idx: usize,
    next_frame_addr: u64,
    free_list: Vec<PhysFrame<Size4KiB>>,
}

impl LimineFrameAllocator {
    /// Создает аллокатор фреймов на базе карты памяти Limine.
    ///
    /// # Safety
    /// Вызывающий код обязан гарантировать, что переданная карта памяти валидна
    /// и получена непосредственно от доверенного загрузчика Limine.
    pub unsafe fn new(entries: &'static [&'static Entry]) -> Self {
        Self {
            entries,
            current_entry_idx: 0,
            next_frame_addr: 0,
            free_list: Vec::new(),
        }
    }

    /// Возвращает физический фрейм обратно в свободный список аллокатора.
    pub fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.free_list.push(frame);
    }

    /// Выделяет `count` физически непрерывных 4 КиБ фреймов.
    /// Критично для аппаратных буферов DMA и кольцевых очередей VirtIO.
    pub fn allocate_contiguous_frames(&mut self, count: usize) -> Option<PhysFrame<Size4KiB>> {
        if count == 0 {
            return None;
        }

        // Если запрошен одиночный фрейм и есть ранее освобожденные страницы
        if count == 1 {
            if let Some(frame) = self.free_list.pop() {
                return Some(frame);
            }
        }

        let needed_bytes = (count as u64) * 4096;

        while self.current_entry_idx < self.entries.len() {
            let entry = self.entries[self.current_entry_idx];

            if entry.type_ != MEMMAP_USABLE {
                self.current_entry_idx += 1;
                self.next_frame_addr = 0;
                continue;
            }

            if self.next_frame_addr == 0 {
                // Выравниваем базовый адрес региона вверх до границы страницы 4 КиБ
                self.next_frame_addr = (entry.base + 4095) & !4095;

                // Нулевая физическая страница (0x0..0x1000) зарезервирована архитектурой x86
                // под IVT реального режима BIOS и BDA. Избегаем её, чтобы не возвращать null-указатели.
                if self.next_frame_addr < 0x1000 {
                    self.next_frame_addr = 0x1000;
                }
            }

            let entry_end = entry.base + entry.length;
            if self.next_frame_addr + needed_bytes <= entry_end {
                let frame_addr = self.next_frame_addr;
                self.next_frame_addr += needed_bytes;
                return Some(PhysFrame::containing_address(PhysAddr::new(frame_addr)));
            }

            // Достигнут конец доступного региона, переходим к следующему
            self.current_entry_idx += 1;
            self.next_frame_addr = 0;
        }

        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for LimineFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate_contiguous_frames(1)
    }
}

impl FrameDeallocator<Size4KiB> for LimineFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.deallocate_frame(frame);
    }
}
