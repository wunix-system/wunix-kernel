use core::alloc::{GlobalAlloc, Layout};
use linked_list_allocator::LockedHeap;
use x86_64::instructions::interrupts;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

// Размещаем кучу ядра в верхней канонической половине виртуального пространства (Higher-Half).
// Диапазон 0x0..0x7FFF_FFFF_FFFF резервируется под будущие процессы пространства пользователя.
// Адрес 0xFFFF_9000_0000_0000 находится выше окна HHDM (0xFFFF_8000_0000_0000) и ниже исполняемого
// кода ядра (-2 GiB, 0xFFFF_FFFF_8000_0000), исключая любые наложения виртуальных страниц.
pub const HEAP_START: usize = 0xFFFF_9000_0000_0000;
pub const HEAP_SIZE: usize = 256 * 1024 * 1024; // 256 MiB

/// Обертка над аллокатором кучи, отключающая прерывания на время операций выделения и освобождения.
/// Без этого прерывание системного таймера (IRQ0) может возникнуть в момент удержания блокировки кучи,
/// попытаться выделить память в обработчике таймера и вызвать глухой дедлок того же ядра процессора.
pub struct InterruptSafeLockedHeap(LockedHeap);

impl InterruptSafeLockedHeap {
    pub const fn empty() -> Self {
        Self(LockedHeap::empty())
    }

    /// Инициализирует внутренний аллокатор кучи с защитой от прерываний.
    ///
    /// # Safety
    /// Вызывающий должен гарантировать, что диапазон памяти валиден и отображен.
    pub unsafe fn init(&self, start: *mut u8, size: usize) {
        interrupts::without_interrupts(|| {
            unsafe {
                self.0.lock().init(start, size);
            }
        });
    }
}

unsafe impl GlobalAlloc for InterruptSafeLockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        interrupts::without_interrupts(|| unsafe {
            GlobalAlloc::alloc(&self.0, layout)
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        interrupts::without_interrupts(|| unsafe {
            GlobalAlloc::dealloc(&self.0, ptr, layout)
        })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        interrupts::without_interrupts(|| unsafe {
            GlobalAlloc::alloc_zeroed(&self.0, layout)
        })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        interrupts::without_interrupts(|| unsafe {
            GlobalAlloc::realloc(&self.0, ptr, layout, new_size)
        })
    }
}

#[global_allocator]
static ALLOCATOR: InterruptSafeLockedHeap = InterruptSafeLockedHeap::empty();

/// Отображает виртуальные страницы диапазона кучи на свободные физические фреймы
/// и инициализирует аллокатор кучи ядра.
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let heap_start = VirtAddr::new(HEAP_START as u64);
    let heap_end = heap_start + (HEAP_SIZE as u64) - 1u64;
    let page_range = {
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    unsafe {
        ALLOCATOR.init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}
