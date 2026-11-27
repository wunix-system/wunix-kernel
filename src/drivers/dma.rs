use core::ops::{Deref, DerefMut};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

/// Освобождает диапазон последовательных 4-КиБ физических фреймов.
/// Вынесено в общий helper, чтобы драйверы без `Dma<T>` не допускали утечек при `Drop`.
pub fn deallocate_contiguous_frames(base: PhysFrame<Size4KiB>, page_count: usize) {
    let mut curr = base;
    for _ in 0..page_count {
        deallocate_frame(curr);
        curr = PhysFrame::containing_address(curr.start_address() + 4096u64);
    }
}

/// Умный DMA-буфер ядра Wunix под типизированный объект `T`.
pub struct Dma<T: ?Sized> {
    phys_frame: PhysFrame<Size4KiB>,
    page_count: usize,
    virt_ptr: *mut T,
}

unsafe impl<T: ?Sized + Send> Send for Dma<T> {}
unsafe impl<T: ?Sized + Sync> Sync for Dma<T> {}

impl<T> Dma<T> {
    /// Выделяет физически непрерывный DMA-буфер под тип `T` и зануляет память.
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

    #[inline]
    pub fn physical(&self) -> usize {
        self.phys_frame.start_address().as_u64() as usize
    }

    #[inline]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

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
        deallocate_contiguous_frames(self.phys_frame, self.page_count);
    }
}

/// Буфер динамического числа непрерывных 4-КиБ DMA-страниц с RAII-освобождением.
/// Используется в очередях, кольцах дескрипторов и фреймбуферах переменного размера.
pub struct DmaPages {
    phys_frame: PhysFrame<Size4KiB>,
    page_count: usize,
    virt_ptr: *mut u8,
}

unsafe impl Send for DmaPages {}
unsafe impl Sync for DmaPages {}

impl DmaPages {
    pub fn zeroed(page_count: usize) -> Result<Self, &'static str> {
        let count = page_count.max(1);
        let phys_frame = allocate_contiguous_frames(count)
            .ok_or("dma: failed to allocate physical contiguous pages")?;
        let virt_ptr = (hhdm_offset() + phys_frame.start_address().as_u64()) as *mut u8;
        unsafe {
            core::ptr::write_bytes(virt_ptr, 0, count * 4096);
        }
        Ok(Self {
            phys_frame,
            page_count: count,
            virt_ptr,
        })
    }

    #[inline]
    pub fn phys_frame(&self) -> PhysFrame<Size4KiB> {
        self.phys_frame
    }

    #[inline]
    pub fn physical(&self) -> u64 {
        self.phys_frame.start_address().as_u64()
    }

    #[inline]
    pub fn virt_ptr(&self) -> *mut u8 {
        self.virt_ptr
    }

    #[inline]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.page_count * 4096
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.virt_ptr, self.byte_len()) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.virt_ptr, self.byte_len()) }
    }
}

impl Drop for DmaPages {
    fn drop(&mut self) {
        deallocate_contiguous_frames(self.phys_frame, self.page_count);
    }
}
