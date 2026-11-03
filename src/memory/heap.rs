use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

// Размещаем кучу ядра в верхней канонической половине виртуального пространства (Higher-Half).
// Диапазон 0x0..0x7FFF_FFFF_FFFF резервируется под будущие процессы пространства пользователя.
// Адрес 0xFFFF_9000_0000_0000 находится выше окна HHDM (0xFFFF_8000_0000_0000) и ниже исполняемого
// кода ядра (-2 GiB, 0xFFFF_FFFF_8000_0000), исключая любые наложения виртуальных страниц.
pub const HEAP_START: usize = 0xFFFF_9000_0000_0000;
pub const HEAP_SIZE: usize = 128 * 1024 * 1024; // 128 MiB

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

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
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}
