use core::ops::{Deref, DerefMut};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

/// Умный DMA-буфер в стиле Redox OS.
/// Выделяет физически непрерывные 4-КиБ страницы для контроллеров PCI (NVMe, AHCI, xHCI, сетевые карты),
/// сохраняя физический адрес (передаваемый в дескрипторы устройства)
/// и отображение в адресное пространство ядра Higher-Half (HHDM) для прямого чтения/записи процессором.
pub struct Dma<T: ?Sized> {
    phys_frame: PhysFrame<Size4KiB>,
    page_count: usize,
    virt_ptr: *mut T,
}

unsafe impl<T: ?Sized + Send> Send for Dma<T> {}
unsafe impl<T: ?Sized + Sync> Sync for Dma<T> {}

impl<T> Dma<T> {
    /// Выделяет DMA буфер под тип T, зануляя память.
    pub fn zeroed() -> Result<Self, &'static str> {
        let size = core::mem::size_of::<T>();
        let page_count = (size + 4095) / 4096;
        let count = page_count.max(1);

        let phys_frame = allocate_contiguous_frames(count)
            .ok_or("dma: failed to allocate physical contiguous frames")?;

        let virt_addr = hhdm_offset() + phys_frame.start_address().as_u64();
        let virt_ptr = virt_addr as *mut T;

        unsafe {
            core::ptr::write_bytes(virt_ptr as *mut u8, 0, count * 4096);
        }

        Ok(Self {
            phys_frame,
            page_count: count,
            virt_ptr,
        })
    }

    /// Физический адрес начала буфера для передачи контроллеру (DMA Base Address)
    #[inline]
    pub fn physical(&self) -> usize {
        self.phys_frame.start_address().as_u64() as usize
    }

    /// Количество выделенных физических страниц по 4 КиБ
    #[inline]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Размер буфера в байтах
    #[inline]
    pub fn size(&self) -> usize {
        self.page_count * 4096
    }
}

impl<T> Deref for Dma<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.virt_ptr }
    }
}

impl<T> DerefMut for Dma<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.virt_ptr }
    }
}

impl<T: ?Sized> Drop for Dma<T> {
    fn drop(&mut self) {
        let mut curr_frame = self.phys_frame;
        for _ in 0..self.page_count {
            deallocate_frame(curr_frame);
            curr_frame = PhysFrame::containing_address(
                curr_frame.start_address() + 4096u64,
            );
        }
    }
}
