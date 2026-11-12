use alloc::vec::Vec;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, Size4KiB, Translate,
};
use x86_64::VirtAddr;

// Права доступа к страницам памяти (POSIX / Linux ABI)
pub const PROT_NONE: u64 = 0x0;
pub const PROT_READ: u64 = 0x1;
pub const PROT_WRITE: u64 = 0x2;
pub const PROT_EXEC: u64 = 0x4;

// Флаги отображения памяти
pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANON: u64 = 0x20;
pub const MAP_ANONYMOUS: u64 = 0x20;

// Границы пользовательского адресного пространства (48-битный канонический диапазон)
pub const USER_SPACE_START: u64 = 0x0000_0000_0040_0000;
pub const VMA_START: u64 = 0x0000_4000_0000_0000;
pub const VMA_END: u64 = 0x0000_7000_0000_0000;
pub const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualArea {
    pub start: u64,
    pub end: u64,
    pub flags: PageTableFlags,
    pub is_committed: bool,
}

#[derive(Clone)]
pub struct VirtualMemoryManager {
    areas: Vec<VirtualArea>,
    next_alloc: u64,
    pub brk_start: u64,
    pub brk_current: u64,
}

impl VirtualMemoryManager {
    pub const fn new() -> Self {
        Self {
            areas: Vec::new(),
            next_alloc: VMA_START,
            brk_start: 0x0000_1000_0000,
            brk_current: 0x0000_1000_0000,
        }
    }

    /// Проверяет, свободен ли виртуальный диапазон [start, start + len) от зарегистрированных VMA
    pub fn is_range_free(&self, start: u64, len: u64) -> bool {
        let end = match start.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        for area in &self.areas {
            if start < area.end && end > area.start {
                return false;
            }
        }
        true
    }

    /// Ищет свободный диапазон виртуальных адресов запрошенной длины
    pub fn find_free_range(&mut self, len: u64) -> Option<u64> {
        let mut candidate = self.next_alloc;
        let mut loop_count = 0;

        while candidate + len <= VMA_END && loop_count < 2 {
            if self.is_range_free(candidate, len) {
                self.next_alloc = candidate + len;
                return Some(candidate);
            }
            // Если диапазон занят, перепрыгиваем за конец пересекающейся области
            let mut jumped = false;
            for area in &self.areas {
                if candidate < area.end && (candidate + len) > area.start {
                    candidate = (area.end + 4095) & !4095;
                    jumped = true;
                    break;
                }
            }
            if !jumped {
                candidate += 4096;
            }
            if candidate + len > VMA_END {
                candidate = VMA_START;
                loop_count += 1;
            }
        }

        None
    }

    /// Регистрирует виртуальную область в менеджере с замещением или объединением
    fn insert_area(&mut self, new_area: VirtualArea) {
        // Удаляем или подрезаем существующие пересекающиеся области
        let mut updated = Vec::new();
        for area in self.areas.drain(..) {
            if area.end <= new_area.start || area.start >= new_area.end {
                updated.push(area);
            } else {
                // Если старая область выступает слева
                if area.start < new_area.start {
                    updated.push(VirtualArea {
                        start: area.start,
                        end: new_area.start,
                        flags: area.flags,
                        is_committed: area.is_committed,
                    });
                }
                // Если старая область выступает справа
                if area.end > new_area.end {
                    updated.push(VirtualArea {
                        start: new_area.end,
                        end: area.end,
                        flags: area.flags,
                        is_committed: area.is_committed,
                    });
                }
            }
        }
        updated.push(new_area);
        updated.sort_by_key(|a| a.start);
        self.areas = updated;
    }

    /// Реализация системного вызова SYS_MMAP (номер 9)
    /// Поддерживает отложенное выделение виртуальных арен (PROT_NONE) и MAP_FIXED для рантайма Go.
    pub fn mmap(
        &mut self,
        addr: u64,
        len: u64,
        prot: u64,
        flags: u64,
        _fd: u64,
        _offset: u64,
    ) -> Result<u64, i64> {
        if len == 0 {
            return Err(-22); // -EINVAL
        }

        // Выравниваем длину вверх до размера 4KiB страницы
        let aligned_len = (len.checked_add(4095).ok_or(-12)? ) & !4095;

        let is_fixed = (flags & MAP_FIXED) != 0;
        let target_addr = if is_fixed {
            if addr % 4096 != 0 || addr < 0x1000 {
                return Err(-22); // -EINVAL: невыровненный или null адрес
            }
            let end_addr = addr.checked_add(aligned_len).ok_or(-22)?;
            if end_addr > USER_SPACE_END {
                return Err(-22); // -EINVAL: выход в пространство ядра
            }
            addr
        } else if addr != 0
            && addr % 4096 == 0
            && addr >= VMA_START
            && addr.checked_add(aligned_len).map_or(false, |e| e <= VMA_END)
            && self.is_range_free(addr, aligned_len)
        {
            addr
        } else {
            self.find_free_range(aligned_len).ok_or(-12)? // -ENOMEM
        };

        // Если запрошен PROT_NONE:
        // Рантайм Go резервирует виртуальные арены (по 64/512 МБ) без расхода физической памяти.
        if prot == PROT_NONE {
            self.insert_area(VirtualArea {
                start: target_addr,
                end: target_addr + aligned_len,
                flags: PageTableFlags::USER_ACCESSIBLE,
                is_committed: false,
            });
            return Ok(target_addr);
        }

        // Выделение физических страниц для PROT_READ / PROT_WRITE
        let mut pt_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if (prot & PROT_WRITE) != 0 {
            pt_flags |= PageTableFlags::WRITABLE;
        }

        let hhdm = super::hhdm_offset();
        let (current_pml4, _) = Cr3::read();
        let pml4_virt = VirtAddr::new(hhdm + current_pml4.start_address().as_u64());

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(target_addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(target_addr + aligned_len - 1));

        for page in Page::range_inclusive(start_page, end_page) {
            let mut mapper = unsafe { OffsetPageTable::new(&mut *pml4_virt.as_mut_ptr(), VirtAddr::new(hhdm)) };
            if let Some(existing_phys) = mapper.translate_addr(page.start_address()) {
                // Если страница уже отображена, обновляем флаги PTE через Mapper и гарантируем права промежуточных таблиц
                unsafe {
                    let _ = mapper.update_flags(page, pt_flags).map(|flush| flush.flush());
                    ensure_parent_tables_writable(pml4_virt, page, hhdm);
                }
                x86_64::instructions::tlb::flush(page.start_address());

                if (flags & MAP_ANON) != 0 {
                    let frame_virt = hhdm + existing_phys.as_u64();
                    unsafe {
                        core::ptr::write_bytes(frame_virt as *mut u8, 0, 4096);
                    }
                }
            } else {
                let frame = super::allocate_frame().ok_or(-12)?; // -ENOMEM
                let frame_virt = hhdm + frame.start_address().as_u64();
                unsafe {
                    core::ptr::write_bytes(frame_virt as *mut u8, 0, 4096);
                }

                let parent_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
                unsafe {
                    mapper
                        .map_to_with_table_flags(page, frame, pt_flags, parent_flags, &mut super::LockedFrameAllocator)
                        .map_err(|_| -12)?
                        .flush();
                    ensure_parent_tables_writable(pml4_virt, page, hhdm);
                }
            }
        }

        self.insert_area(VirtualArea {
            start: target_addr,
            end: target_addr + aligned_len,
            flags: pt_flags,
            is_committed: true,
        });

        Ok(target_addr)
    }

    /// Реализация системного вызова SYS_MUNMAP (номер 11)
    pub fn munmap(&mut self, addr: u64, len: u64) -> Result<u64, i64> {
        if addr % 4096 != 0 || len == 0 {
            return Err(-22); // -EINVAL
        }

        let aligned_len = (len.checked_add(4095).ok_or(-12)? ) & !4095;
        let end_addr = match addr.checked_add(aligned_len) {
            Some(e) if e <= USER_SPACE_END => e,
            _ => return Err(-22),
        };

        let hhdm = super::hhdm_offset();
        let (current_pml4, _) = Cr3::read();
        let pml4_virt = VirtAddr::new(hhdm + current_pml4.start_address().as_u64());
        let pml4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        let mut mapper = unsafe { OffsetPageTable::new(pml4_table, VirtAddr::new(hhdm)) };

        for page in Page::range_inclusive(start_page, end_page) {
            if mapper.translate_page(page).is_ok() {
                if let Ok((phys_frame, flush)) = mapper.unmap(page) {
                    flush.flush();
                    super::deallocate_frame(phys_frame);
                }
            }
        }

        // Вырезаем диапазон [addr, end_addr) из списка VMA
        let mut updated = Vec::new();
        for area in self.areas.drain(..) {
            if area.end <= addr || area.start >= end_addr {
                updated.push(area);
            } else {
                if area.start < addr {
                    updated.push(VirtualArea {
                        start: area.start,
                        end: addr,
                        flags: area.flags,
                        is_committed: area.is_committed,
                    });
                }
                if area.end > end_addr {
                    updated.push(VirtualArea {
                        start: end_addr,
                        end: area.end,
                        flags: area.flags,
                        is_committed: area.is_committed,
                    });
                }
            }
        }
        self.areas = updated;

        Ok(0)
    }

    /// Реализация системного вызова SYS_MPROTECT (номер 10)
    pub fn mprotect(&mut self, addr: u64, len: u64, prot: u64) -> Result<u64, i64> {
        if addr % 4096 != 0 || len == 0 {
            return Err(-22); // -EINVAL
        }

        let aligned_len = (len.checked_add(4095).ok_or(-12)? ) & !4095;
        let end_addr = match addr.checked_add(aligned_len) {
            Some(e) if e <= USER_SPACE_END => e,
            _ => return Err(-22),
        };

        let mut pt_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if (prot & PROT_WRITE) != 0 {
            pt_flags |= PageTableFlags::WRITABLE;
        }

        let hhdm = super::hhdm_offset();
        let (current_pml4, _) = Cr3::read();
        let pml4_virt = VirtAddr::new(hhdm + current_pml4.start_address().as_u64());

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

        for page in Page::range_inclusive(start_page, end_page) {
            let mut mapper = unsafe { OffsetPageTable::new(&mut *pml4_virt.as_mut_ptr(), VirtAddr::new(hhdm)) };
            if mapper.translate_page(page).is_ok() {
                unsafe {
                    let _ = mapper.update_flags(page, pt_flags).map(|flush| flush.flush());
                    if pt_flags.contains(PageTableFlags::WRITABLE) {
                        ensure_parent_tables_writable(pml4_virt, page, hhdm);
                    }
                }
                x86_64::instructions::tlb::flush(page.start_address());
            }
        }

        for area in &mut self.areas {
            if area.start >= addr && area.end <= end_addr {
                area.flags = pt_flags;
            }
        }

        Ok(0)
    }

    /// Реализация системного вызова SYS_BRK (номер 12)
    pub fn brk(&mut self, new_brk: u64) -> u64 {
        if new_brk == 0 || new_brk < self.brk_start || new_brk > USER_SPACE_END {
            return self.brk_current;
        }

        let cur_aligned = (self.brk_current + 4095) & !4095;
        let new_aligned = (new_brk + 4095) & !4095;

        if new_aligned > cur_aligned {
            let size = new_aligned - cur_aligned;
            if self.mmap(cur_aligned, size, PROT_READ | PROT_WRITE, MAP_FIXED | MAP_ANON, 0, 0).is_err() {
                return self.brk_current;
            }
        } else if new_aligned < cur_aligned {
            let size = cur_aligned - new_aligned;
            let _ = self.munmap(new_aligned, size);
        }

        self.brk_current = new_brk;
        self.brk_current
    }

    /// Сбрасывает VMA-менеджер при запуске нового процесса (execve)
    pub fn reset(&mut self, brk_start: u64) {
        self.areas.clear();
        self.next_alloc = VMA_START;
        self.brk_start = brk_start;
        self.brk_current = brk_start;
    }

    /// Проверяет, принадлежит ли виртуальный адрес валидному диапазону процесса (сегменты ELF, куча или VMA)
    pub fn is_valid_addr(&self, addr: u64) -> bool {
        let brk_top = (self.brk_current + 4095) & !4095;
        if addr >= 0x40_0000 && addr < brk_top {
            return true;
        }
        for area in &self.areas {
            if addr >= area.start && addr < area.end {
                return true;
            }
        }
        false
    }
}

/// Гарантирует, что промежуточные таблицы страниц (PML4, PDPT, PD) имеют флаги USER_ACCESSIBLE и WRITABLE,
/// исключая аппаратные сбои Ring 3 при обновлении листовых записей PTE на запись.
pub unsafe fn ensure_parent_tables_writable(
    pml4_virt: VirtAddr,
    page: Page<Size4KiB>,
    hhdm: u64,
) {
    let level_4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };
    let p4_entry = &mut level_4_table[page.p4_index()];
    if p4_entry.is_unused() {
        return;
    }
    p4_entry.set_flags(p4_entry.flags() | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE);

    let p3_virt = VirtAddr::new(hhdm + p4_entry.addr().as_u64());
    let p3_table: &mut PageTable = unsafe { &mut *p3_virt.as_mut_ptr() };
    let p3_entry = &mut p3_table[page.p3_index()];
    if p3_entry.is_unused() {
        return;
    }
    p3_entry.set_flags(p3_entry.flags() | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE);

    let p2_virt = VirtAddr::new(hhdm + p3_entry.addr().as_u64());
    let p2_table: &mut PageTable = unsafe { &mut *p2_virt.as_mut_ptr() };
    let p2_entry = &mut p2_table[page.p2_index()];
    if p2_entry.is_unused() {
        return;
    }
    p2_entry.set_flags(p2_entry.flags() | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE);
}
