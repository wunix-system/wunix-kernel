use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{OffsetPageTable, PageTable};
use x86_64::VirtAddr;

/// Инициализирует транслятор страниц OffsetPageTable на базе HHDM.
///
/// # Safety
/// Вызывающий код обязан гарантировать:
/// 1. `hhdm_offset` — корректное смещение прямого отображения физической памяти,
///    настроенное загрузчиком Limine.
/// 2. Корневая таблица страниц 4-го уровня (PML4) в CR3 валидна.
pub unsafe fn init_offset_page_table(hhdm_offset: u64) -> OffsetPageTable<'static> {
    // В CR3 хранится исключительно физический адрес PML4.
    // Так как ядро выполняется в Higher-Half с включенным пейджингом,
    // прямой доступ по физическому адресу вызовет Page Fault.
    // Используем окно HHDM (Higher-Half Direct Map), куда Limine спроецировал всю физическую память.
    let (level_4_table_frame, _) = Cr3::read();
    let phys_addr = level_4_table_frame.start_address();
    let virt_addr = VirtAddr::new(hhdm_offset + phys_addr.as_u64());
    let page_table_ptr: *mut PageTable = virt_addr.as_mut_ptr();

    let level_4_table: &'static mut PageTable = unsafe { &mut *page_table_ptr };
    unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm_offset)) }
}
