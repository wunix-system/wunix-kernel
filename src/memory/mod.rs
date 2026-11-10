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
static GLOBAL_MEMORY_STATS: Mutex<Option<MemoryStats>> = Mutex::new(None);

pub fn total_memory_bytes() -> u64 {
    GLOBAL_MEMORY_STATS.lock().map_or(512 * 1024 * 1024, |s| s.ram_bytes)
}

pub fn free_memory_bytes() -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| {
        FRAME_ALLOCATOR.lock().as_ref().map_or(450 * 1024 * 1024, |a| a.free_bytes())
    })
}

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

        match mapper.translate_page(page) {
            Ok(_) | Err(x86_64::structures::paging::mapper::TranslateError::ParentEntryHugePage) => {
                // Страница уже отображена в HHDM ядра (напрямую или через 1GB/2MB Huge Pages Limine)
            }
            Err(_) => {
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
    *GLOBAL_MEMORY_STATS.lock() = Some(collect_memory_stats(memory_map));

    let mut mapper = unsafe { paging::init_offset_page_table(hhdm_offset) };
    let frame_allocator = unsafe { frame::LimineFrameAllocator::new(memory_map) };
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);

    let mut locked_allocator = LockedFrameAllocator;
    heap::init_heap(&mut mapper, &mut locked_allocator)
        .expect("Failed to initialize kernel heap");
}

/// Обрабатывает пользовательский Page Fault (#PF) в Ring 3:
/// Выполняет отложенное выделение физических страниц (demand paging) для стека пользователя,
/// диапазона кучи brk и зарегистрированных виртуальных арен VMA.
pub fn handle_user_page_fault(
    addr: VirtAddr,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) -> Result<(), ()> {
    use x86_64::structures::idt::PageFaultErrorCode;
    if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
        return Err(());
    }

    let addr_u64 = addr.as_u64();
    if addr_u64 >= 0x0000_8000_0000_0000 || addr_u64 < 0x1000 {
        return Err(());
    }

    let is_stack = addr_u64 >= (crate::task::elf::USER_STACK_TOP.saturating_sub(8 * 1024 * 1024))
        && addr_u64 < crate::task::elf::USER_STACK_TOP;
    let is_vma = crate::task::syscall::mem::is_valid_user_vma_addr(addr_u64);

    if !is_stack && !is_vma {
        return Err(());
    }

    let page = Page::<Size4KiB>::containing_address(addr);
    let hhdm = hhdm_offset();
    let (current_pml4, _) = Cr3::read();
    let pml4_virt = VirtAddr::new(hhdm + current_pml4.start_address().as_u64());
    let pml4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };
    let mut mapper = unsafe { OffsetPageTable::new(pml4_table, VirtAddr::new(hhdm)) };

    let frame = allocate_frame().ok_or(())?;
    let frame_virt = hhdm + frame.start_address().as_u64();
    unsafe {
        core::ptr::write_bytes(frame_virt as *mut u8, 0, 4096);
    }

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    unsafe {
        mapper
            .map_to(page, frame, flags, &mut LockedFrameAllocator)
            .map_err(|_| ())?
            .flush();
    }

    let pml4_idx = page.start_address().p4_index();
    let cur = pml4_table[pml4_idx].flags();
    pml4_table[pml4_idx].set_flags(cur | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE);

    Ok(())
}

/// Транслирует виртуальный адрес в физический через текущую активную таблицу страниц (CR3).
pub fn translate_address(addr: u64) -> Option<u64> {
    use x86_64::structures::paging::Translate;
    let hhdm = hhdm_offset();
    if hhdm == 0 {
        return None;
    }
    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = VirtAddr::new(phys.as_u64() + hhdm);
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    let level_4_table = unsafe { &mut *page_table_ptr };
    let mapper = unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm)) };
    mapper.translate_addr(VirtAddr::new(addr)).map(|p| p.as_u64())
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
