use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::VirtAddr;

pub mod frame;
pub mod heap;
pub mod paging;
pub mod process;
pub mod vma;

pub use frame::LimineFrameAllocator;
pub use process::UserAddressSpace;
pub use vma::VirtualMemoryManager;

pub mod memory_map {
    pub use limine::memmap::Entry;
    pub struct EntryType;
    impl EntryType {
        pub const USABLE: u64 = limine::memmap::MEMMAP_USABLE;
    }
}
pub type MemoryMapRequest = limine::request::MemmapRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub usable_bytes: u64,
    pub ram_bytes: u64,
    pub entry_count: usize,
    pub usable_regions: usize,
}

impl MemoryStats {
    pub const fn total_pages(&self) -> u64 {
        self.total_bytes / 4096
    }

    pub const fn usable_pages(&self) -> u64 {
        self.usable_bytes / 4096
    }

    pub const fn total_mb(&self) -> u64 {
        self.total_bytes / (1024 * 1024)
    }

    pub const fn usable_mb(&self) -> u64 {
        self.usable_bytes / (1024 * 1024)
    }

    pub const fn ram_mb(&self) -> u64 {
        self.ram_bytes / (1024 * 1024)
    }
}

/// Агрегирует статистику физической памяти ядра по дескрипторам карты Limine.
pub fn collect_memory_stats(entries: &[&limine::memmap::Entry]) -> MemoryStats {
    use limine::memmap::*;

    let mut total_bytes = 0u64;
    let mut usable_bytes = 0u64;
    let mut ram_bytes = 0u64;
    let mut usable_regions = 0usize;

    for entry in entries {
        total_bytes += entry.length;

        if entry.type_ == MEMMAP_USABLE {
            usable_bytes += entry.length;
            usable_regions += 1;
        }

        // Физическая оперативная память компьютера (исключая MMIO дыры и зарезервированные ROM)
        if entry.type_ == MEMMAP_USABLE
            || entry.type_ == MEMMAP_BOOTLOADER_RECLAIMABLE
            || entry.type_ == MEMMAP_EXECUTABLE_AND_MODULES
            || entry.type_ == MEMMAP_ACPI_RECLAIMABLE
        {
            ram_bytes += entry.length;
        }
    }

    MemoryStats {
        total_bytes,
        usable_bytes,
        ram_bytes,
        entry_count: entries.len(),
        usable_regions,
    }
}

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);
static FRAME_ALLOCATOR: Mutex<Option<LimineFrameAllocator>> = Mutex::new(None);

pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        FRAME_ALLOCATOR.lock().as_mut()?.allocate_frame()
    })
}

pub fn deallocate_frame(frame: PhysFrame<Size4KiB>) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(alloc) = FRAME_ALLOCATOR.lock().as_mut() {
            alloc.deallocate_frame(frame);
        }
    });
}

pub fn allocate_contiguous_frames(count: usize) -> Option<PhysFrame<Size4KiB>> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        FRAME_ALLOCATOR.lock().as_mut()?.allocate_contiguous_frames(count)
    })
}

pub struct LockedFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for LockedFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        crate::memory::allocate_frame()
    }
}

impl x86_64::structures::paging::FrameDeallocator<Size4KiB> for LockedFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        crate::memory::deallocate_frame(frame);
    }
}

/// Отображает физический диапазон памяти MMIO (PCI BAR) в окно Higher-Half ядра.
/// Если страницы ещё не отображены в таблицах PML4, создает соответствующие записи
/// с флагами NO_CACHE и WRITE_THROUGH для предотвращения кэширования регистров железа.
pub fn map_kernel_mmio(phys_addr: u64, size: usize) -> Result<VirtAddr, &'static str> {
    if size == 0 {
        return Ok(VirtAddr::new(hhdm_offset() + phys_addr));
    }

    // Защита от поврежденных или фиктивных BAR-регистров: ядро не должно пытаться
    // аллоцировать таблицы страниц под гигантские некорректные диапазоны (например > 256 МБ).
    if size > 256 * 1024 * 1024 {
        return Err("map_kernel_mmio: requested size exceeds maximum limit of 256 MiB");
    }

    let hhdm = hhdm_offset();
    let (kernel_pml4_frame, _) = Cr3::read();
    let kernel_pml4_virt = VirtAddr::new(hhdm + kernel_pml4_frame.start_address().as_u64());
    let level_4_table: &mut PageTable = unsafe { &mut *(kernel_pml4_virt.as_mut_ptr()) };

    let mut mapper = unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm)) };
    let mut allocator = LockedFrameAllocator;

    let start_phys = phys_addr & !0xFFF;
    let end_phys = (phys_addr + (size as u64) - 1) & !0xFFF;

    let mut curr_phys = start_phys;
    while curr_phys <= end_phys {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(hhdm + curr_phys));
        let frame: PhysFrame<Size4KiB> =
            PhysFrame::containing_address(x86_64::PhysAddr::new(curr_phys));

        if mapper.translate_page(page).is_err() {
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_CACHE
                | PageTableFlags::WRITE_THROUGH;

            unsafe {
                mapper
                    .map_to(page, frame, flags, &mut allocator)
                    .map_err(|_| "Failed to map MMIO page")?
                    .flush();
            }
        }

        curr_phys += 4096;
    }

    Ok(VirtAddr::new(hhdm + phys_addr))
}

/// Инициализирует подсистему управления памятью:
/// - Сохраняет HHDM смещение и инициализирует глобальный аллокатор фреймов.
/// - Подключает трансляцию виртуальных адресов OffsetPageTable через окно HHDM.
/// - Выделяет и маппит страницы под кучу ядра (1 MiB) и активирует аллокатор.
pub fn init(memory_map: &'static [&'static limine::memmap::Entry], hhdm_offset: u64) {
    HHDM_OFFSET.store(hhdm_offset, Ordering::Relaxed);

    let mut mapper = unsafe { paging::init_offset_page_table(hhdm_offset) };
    let frame_allocator = unsafe { frame::LimineFrameAllocator::new(memory_map) };
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);

    let mut locked_allocator = LockedFrameAllocator;
    heap::init_heap(&mut mapper, &mut locked_allocator)
        .expect("Failed to initialize kernel heap");
}

#[cfg(test)]
mod tests {
    use super::*;
    use limine::memmap::{Entry, MEMMAP_RESERVED, MEMMAP_USABLE};

    #[test_case]
    fn test_memory_stats_calculation() {
        static E1: Entry = Entry {
            base: 0x1000,
            length: 4 * 1024 * 1024,
            type_: MEMMAP_USABLE,
        };
        static E2: Entry = Entry {
            base: 0x500_0000,
            length: 2 * 1024 * 1024,
            type_: MEMMAP_RESERVED,
        };
        static E3: Entry = Entry {
            base: 0x800_0000,
            length: 8 * 1024 * 1024,
            type_: MEMMAP_USABLE,
        };

        let entries: [&'static Entry; 3] = [&E1, &E2, &E3];
        let stats = collect_memory_stats(&entries);

        assert_eq!(stats.total_bytes, 14 * 1024 * 1024);
        assert_eq!(stats.usable_bytes, 12 * 1024 * 1024);
        assert_eq!(stats.ram_bytes, 12 * 1024 * 1024);
        assert_eq!(stats.entry_count, 3);
        assert_eq!(stats.usable_regions, 2);
        assert_eq!(stats.total_mb(), 14);
        assert_eq!(stats.usable_mb(), 12);
        assert_eq!(stats.ram_mb(), 12);
        assert_eq!(stats.total_pages(), (14 * 1024 * 1024) / 4096);
        assert_eq!(stats.usable_pages(), (12 * 1024 * 1024) / 4096);
    }
}
