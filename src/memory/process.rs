use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::VirtAddr;

/// Представляет изолированное виртуальное адресное пространство процесса (таблица PML4).
#[derive(Debug)]
pub struct UserAddressSpace {
    /// Физический фрейм корневой таблицы страниц PML4 (загружается в CR3)
    pub pml4_frame: PhysFrame<Size4KiB>,
}

impl UserAddressSpace {
    /// Создает новое изолированное адресное пространство процесса:
    /// 1. Выделяет физический фрейм для PML4 таблицы процесса.
    /// 2. Очищает нижнюю половину (индексы 0..255: User Space).
    /// 3. Копирует записи верхней половины (индексы 256..511: Higher-Half ядра),
    ///    чтобы код ядра, HHDM, стек и куча оставались валидными при системных вызовах и IRQ.
    pub fn new() -> Result<Self, &'static str> {
        let pml4_frame = super::allocate_frame().ok_or("Не удалось выделить фрейм для PML4")?;

        let hhdm = super::hhdm_offset();
        let new_pml4_virt = VirtAddr::new(hhdm + pml4_frame.start_address().as_u64());
        let new_table: &mut PageTable = unsafe { &mut *new_pml4_virt.as_mut_ptr() };

        // Обнуляем все 512 записей новой таблицы страниц
        new_table.zero();

        // Получаем доступ к текущей таблице страниц ядра через HHDM
        let (kernel_pml4_frame, _) = Cr3::read();
        let kernel_pml4_virt = VirtAddr::new(hhdm + kernel_pml4_frame.start_address().as_u64());
        let kernel_table: &PageTable = unsafe { &*kernel_pml4_virt.as_ptr() };

        // Старшая половина (Higher-Half, индексы 256..511) копируется из ядра.
        // Записи каталога общие, поэтому отображения ядра и HHDM идентичны во всех процессах.
        for i in 256..512 {
            new_table[i] = kernel_table[i].clone();
        }

        Ok(Self { pml4_frame })
    }

    /// Отображает виртуальную страницу на конкретный физический фрейм с заданными флагами.
    pub fn map_page(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        let hhdm = super::hhdm_offset();
        let pml4_virt = VirtAddr::new(hhdm + self.pml4_frame.start_address().as_u64());
        let level_4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };

        let mut mapper = unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm)) };
        let mut allocator = super::LockedFrameAllocator;

        let parent_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        unsafe {
            mapper
                .map_to_with_table_flags(page, frame, flags, parent_flags, &mut allocator)
                .map_err(|_| "Ошибка маппинга страницы в адресное пространство процесса")?
                .flush();
        }

        // Гарантируем, что запись PML4 имеет флаг USER_ACCESSIBLE, если страница пользовательская.
        // В x86_64 без бита User на верхних уровнях процессор запрещает доступ из Ring 3.
        if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            let pml4_idx = page.start_address().p4_index();
            let current_flags = level_4_table[pml4_idx].flags();
            level_4_table[pml4_idx].set_flags(
                current_flags | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE,
            );
        }

        Ok(())
    }

    /// Загружает сегмент ELF (PT_LOAD) в виртуальную память процесса.
    /// Выделяет фреймы, копирует байты из `file_data` по смещению `vaddr`,
    /// а оставшуюся часть до `memsz` оставляет зануленной (BSS).
    pub fn load_segment(
        &mut self,
        vaddr: u64,
        memsz: u64,
        file_data: &[u8],
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        if memsz == 0 {
            return Ok(());
        }

        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr + memsz - 1));
        let page_range = Page::range_inclusive(start_page, end_page);

        let hhdm = super::hhdm_offset();
        let pml4_virt = VirtAddr::new(hhdm + self.pml4_frame.start_address().as_u64());
        let level_4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };

        for page in page_range {
            let page_start = page.start_address().as_u64();
            let page_end = page_start + 4096;

            let mut mapper = unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm)) };

            // Проверяем, отображена ли страница уже в данном адресном пространстве
            use x86_64::structures::paging::Translate;
            let frame = if let Some(phys) = mapper.translate_addr(page.start_address()) {
                PhysFrame::<Size4KiB>::containing_address(phys)
            } else {
                let new_frame = super::allocate_frame().ok_or("Недостаточно физической памяти")?;
                let frame_virt = VirtAddr::new(hhdm + new_frame.start_address().as_u64());

                // Очищаем фрейм нулями перед использованием
                unsafe {
                    core::ptr::write_bytes(frame_virt.as_mut_ptr::<u8>(), 0, 4096);
                }

                let parent_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
                unsafe {
                    mapper
                        .map_to_with_table_flags(page, new_frame, flags, parent_flags, &mut super::LockedFrameAllocator)
                        .map_err(|_| "Ошибка маппинга страницы сегмента ELF")?
                        .flush();
                }

                // Гарантируем права доступа пользователя и записи на верхнем уровне PML4
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                    let pml4_idx = page.start_address().p4_index();
                    let cur = level_4_table[pml4_idx].flags();
                    level_4_table[pml4_idx].set_flags(
                        cur | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE,
                    );
                }

                new_frame
            };

            // Копируем пересекающуюся порцию данных файла в физический фрейм
            let copy_vstart = vaddr.max(page_start);
            let copy_vend = (vaddr + file_data.len() as u64).min(page_end);

            if copy_vstart < copy_vend {
                let copy_len = (copy_vend - copy_vstart) as usize;
                let file_offset = (copy_vstart - vaddr) as usize;
                let page_offset = (copy_vstart - page_start) as usize;

                let dst = (hhdm + frame.start_address().as_u64() + page_offset as u64) as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        file_data[file_offset..].as_ptr(),
                        dst,
                        copy_len,
                    );
                }
            }
        }

        Ok(())
    }

    /// Выделяет физические фреймы и отображает диапазон виртуальных адресов пользователя.
    /// Память выделенных страниц обнуляется.
    pub fn allocate_and_map_range(
        &mut self,
        start_addr: VirtAddr,
        size_bytes: usize,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        let start_page = Page::<Size4KiB>::containing_address(start_addr);
        let end_page = Page::<Size4KiB>::containing_address(start_addr + (size_bytes as u64) - 1u64);
        let page_range = Page::range_inclusive(start_page, end_page);

        let hhdm = super::hhdm_offset();

        for page in page_range {
            let frame = super::allocate_frame().ok_or("Недостаточно физической памяти")?;

            // Очищаем физическую страницу через виртуальное окно HHDM
            let frame_virt = VirtAddr::new(hhdm + frame.start_address().as_u64());
            unsafe {
                core::ptr::write_bytes(frame_virt.as_mut_ptr::<u8>(), 0, 4096);
            }

            self.map_page(page, frame, flags)?;
        }

        Ok(())
    }

    /// Записывает срез байтов `data` в виртуальное адресное пространство процесса по адресу `vaddr`.
    /// Использует HHDM для прямого доступа к физическим фреймам без необходимости временного переключения CR3.
    pub fn write_user_bytes(&mut self, vaddr: u64, data: &[u8]) -> Result<(), &'static str> {
        let hhdm = super::hhdm_offset();
        let pml4_virt = VirtAddr::new(hhdm + self.pml4_frame.start_address().as_u64());
        let level_4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };
        let mapper = unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(hhdm)) };

        use x86_64::structures::paging::Translate;

        let mut written = 0;
        while written < data.len() {
            let cur_vaddr = vaddr + written as u64;
            let page_offset = (cur_vaddr % 4096) as usize;
            let chunk_len = (4096 - page_offset).min(data.len() - written);

            let phys = mapper
                .translate_addr(VirtAddr::new(cur_vaddr))
                .ok_or("Unmapped user virtual address")?;
            let k_dst = (hhdm + phys.as_u64()) as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(data[written..].as_ptr(), k_dst, chunk_len);
            }
            written += chunk_len;
        }
        Ok(())
    }

    /// Отображает переданный срез машинного кода в виртуальную память процесса по адресу `start_addr`.
    /// Выделяет необходимые физические фреймы и копирует байты кода.
    pub fn map_user_code_slice(
        &mut self,
        start_addr: VirtAddr,
        code: &[u8],
    ) -> Result<(), &'static str> {
        let size_bytes = code.len();
        if size_bytes == 0 {
            return Ok(());
        }

        let start_page = Page::<Size4KiB>::containing_address(start_addr);
        let end_page =
            Page::<Size4KiB>::containing_address(start_addr + (size_bytes as u64) - 1u64);
        let page_range = Page::range_inclusive(start_page, end_page);

        let hhdm = super::hhdm_offset();
        let mut offset = 0;

        for page in page_range {
            let frame = super::allocate_frame().ok_or("Недостаточно физической памяти")?;
            let frame_virt = VirtAddr::new(hhdm + frame.start_address().as_u64());

            // Очищаем фрейм
            unsafe {
                core::ptr::write_bytes(frame_virt.as_mut_ptr::<u8>(), 0, 4096);
            }

            // Копируем кусок кода (до 4096 байт) во фрейм
            let chunk_len = (size_bytes - offset).min(4096);
            if chunk_len > 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        code.as_ptr().add(offset),
                        frame_virt.as_mut_ptr::<u8>(),
                        chunk_len,
                    );
                }
                offset += chunk_len;
            }

            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE;

            self.map_page(page, frame, flags)?;
        }

        Ok(())
    }

    /// Создаёт полную изолированную копию адресного пространства процесса по его физическому CR3
    /// (для `fork` / `clone` без `CLONE_VM`). Копирует все пользовательские страницы (PML4 0..256)
    /// в новые физические фреймы, сохраняя флаги доступа.
    pub fn clone_from_cr3(parent_cr3: u64) -> Result<Self, &'static str> {
        let mut child_space = Self::new()?;
        let hhdm = super::hhdm_offset();
        let parent_pml4_phys = parent_cr3 & !0xFFF;
        let parent_pml4: &PageTable = unsafe { &*((hhdm + parent_pml4_phys) as *const PageTable) };
        let child_pml4_phys = child_space.pml4_frame.start_address().as_u64();
        let child_pml4: &mut PageTable = unsafe { &mut *((hhdm + child_pml4_phys) as *mut PageTable) };

        for p4_idx in 0..256 {
            let p4_entry = &parent_pml4[p4_idx];
            if !p4_entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let new_p3_frame = Self::clone_pdpt(p4_entry.addr().as_u64(), hhdm)?;
            child_pml4[p4_idx].set_frame(new_p3_frame, p4_entry.flags());
        }

        // Возвращаем ownership наружу, чтобы CR3 остался валидным
        let _ = &mut child_space;
        Ok(child_space)
    }

    fn alloc_zeroed_table(hhdm: u64) -> Result<(PhysFrame<Size4KiB>, &'static mut PageTable), &'static str> {
        let frame = super::allocate_frame().ok_or("Недостаточно физической памяти для таблицы страниц")?;
        let virt = VirtAddr::new(hhdm + frame.start_address().as_u64());
        let table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
        table.zero();
        Ok((frame, table))
    }

    fn clone_pdpt(parent_pdpt_phys: u64, hhdm: u64) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let parent_pdpt: &PageTable = unsafe { &*((hhdm + parent_pdpt_phys) as *const PageTable) };
        let (new_frame, new_pdpt) = Self::alloc_zeroed_table(hhdm)?;
        for idx in 0..512 {
            let entry = &parent_pdpt[idx];
            if !entry.flags().contains(PageTableFlags::PRESENT) || entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                continue;
            }
            let new_p2 = Self::clone_pd(entry.addr().as_u64(), hhdm)?;
            new_pdpt[idx].set_frame(new_p2, entry.flags());
        }
        Ok(new_frame)
    }

    fn clone_pd(parent_pd_phys: u64, hhdm: u64) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let parent_pd: &PageTable = unsafe { &*((hhdm + parent_pd_phys) as *const PageTable) };
        let (new_frame, new_pd) = Self::alloc_zeroed_table(hhdm)?;
        for idx in 0..512 {
            let entry = &parent_pd[idx];
            if !entry.flags().contains(PageTableFlags::PRESENT) || entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                continue;
            }
            let new_p1 = Self::clone_pt(entry.addr().as_u64(), hhdm)?;
            new_pd[idx].set_frame(new_p1, entry.flags());
        }
        Ok(new_frame)
    }

    fn clone_pt(parent_pt_phys: u64, hhdm: u64) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let parent_pt: &PageTable = unsafe { &*((hhdm + parent_pt_phys) as *const PageTable) };
        let (new_frame, new_pt) = Self::alloc_zeroed_table(hhdm)?;
        for idx in 0..512 {
            let entry = &parent_pt[idx];
            if !entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let dst_page_frame = super::allocate_frame().ok_or("Недостаточно физической памяти для копирования страницы")?;
            let src_ptr = (hhdm + entry.addr().as_u64()) as *const u8;
            let dst_ptr = (hhdm + dst_page_frame.start_address().as_u64()) as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, 4096);
            }
            new_pt[idx].set_frame(dst_page_frame, entry.flags());
        }
        Ok(new_frame)
    }

    /// Передаёт владение физическим CR3 наружу без освобождения фреймов при drop.
    pub fn into_raw_cr3(self) -> u64 {
        let cr3 = self.pml4_frame.start_address().as_u64();
        core::mem::forget(self);
        cr3
    }

    /// Активирует данное адресное пространство, загружая физический адрес PML4 в регистр CR3.
    pub fn activate(&self) {
        unsafe {
            Cr3::write(self.pml4_frame, Cr3Flags::empty());
        }
    }
}

impl Drop for UserAddressSpace {
    fn drop(&mut self) {
        unsafe {
            free_user_address_space(self.pml4_frame.start_address().as_u64());
        }
    }
}

/// Освобождает все пользовательские физические страницы (PML4 0..256),
/// промежуточные таблицы PDPT/PD/PT и сам фрейм PML4.
/// Глобальные разделяемые фреймы syslib пропускаются.
pub unsafe fn free_user_address_space(pml4_phys: u64) {
    let pml4_addr = pml4_phys & !0xFFF;
    let kernel_pml4 = super::kernel_cr3() & !0xFFF;
    if pml4_addr == 0 || (kernel_pml4 != 0 && pml4_addr == kernel_pml4) {
        return;
    }

    let (cur_cr3, _) = Cr3::read();
    if cur_cr3.start_address().as_u64() == pml4_addr && kernel_pml4 != 0 {
        let kframe = x86_64::structures::paging::PhysFrame::containing_address(
            x86_64::PhysAddr::new(kernel_pml4),
        );
        unsafe {
            Cr3::write(kframe, Cr3Flags::empty());
        }
    }

    let hhdm = super::hhdm_offset();
    let pml4: &PageTable = unsafe { &*((hhdm + pml4_addr) as *const PageTable) };

    for p4_idx in 0..256 {
        let p4_entry = &pml4[p4_idx];
        if !p4_entry.flags().contains(PageTableFlags::PRESENT)
            || p4_entry.flags().contains(PageTableFlags::HUGE_PAGE)
        {
            continue;
        }
        let pdpt_phys = p4_entry.addr().as_u64();
        free_pdpt(pdpt_phys, hhdm);
    }

    super::deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pml4_addr)));
}

fn free_pdpt(pdpt_phys: u64, hhdm: u64) {
    let pdpt: &PageTable = unsafe { &*((hhdm + pdpt_phys) as *const PageTable) };
    for entry in pdpt.iter() {
        if !entry.flags().contains(PageTableFlags::PRESENT)
            || entry.flags().contains(PageTableFlags::HUGE_PAGE)
        {
            continue;
        }
        free_pd(entry.addr().as_u64(), hhdm);
    }
    super::deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pdpt_phys)));
}

fn free_pd(pd_phys: u64, hhdm: u64) {
    let pd: &PageTable = unsafe { &*((hhdm + pd_phys) as *const PageTable) };
    for entry in pd.iter() {
        if !entry.flags().contains(PageTableFlags::PRESENT)
            || entry.flags().contains(PageTableFlags::HUGE_PAGE)
        {
            continue;
        }
        free_pt(entry.addr().as_u64(), hhdm);
    }
    super::deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pd_phys)));
}

fn free_pt(pt_phys: u64, hhdm: u64) {
    let pt: &PageTable = unsafe { &*((hhdm + pt_phys) as *const PageTable) };
    for entry in pt.iter() {
        if !entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        if let Ok(frame) = entry.frame() {
            if !crate::task::syslib::is_syslib_frame(frame) {
                super::deallocate_frame(frame);
            }
        }
    }
    super::deallocate_frame(PhysFrame::containing_address(x86_64::PhysAddr::new(pt_phys)));
}
