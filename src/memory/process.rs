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

        unsafe {
            mapper
                .map_to(page, frame, flags, &mut allocator)
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

                unsafe {
                    mapper
                        .map_to(page, new_frame, flags, &mut super::LockedFrameAllocator)
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

    /// Активирует данное адресное пространство, загружая физический адрес PML4 в регистр CR3.
    pub fn activate(&self) {
        unsafe {
            Cr3::write(self.pml4_frame, Cr3Flags::empty());
        }
    }
}
