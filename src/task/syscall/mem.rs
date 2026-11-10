use super::{
    validate_user_buffer, VMA_MANAGER, EFAULT, EINVAL, USER_SPACE_END,
};

pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, offset: u64) -> u64 {
    const MAP_ANONYMOUS: u64 = 0x20;
    let is_file_backed = (flags & MAP_ANONYMOUS) == 0 && (fd as i64) >= 0;
    let file_inode = if is_file_backed {
        crate::fs::get_file_handle(fd).and_then(|h| h.as_inode())
    } else {
        None
    };

    let target_addr = {
        let mut vma = VMA_MANAGER.lock();
        match vma.mmap(addr, len, prot, flags, fd, offset) {
            Ok(vaddr) => vaddr,
            Err(err) => return (-(err.abs())) as u64,
        }
    };

    if let Some(inode) = file_inode {
        if prot != crate::memory::vma::PROT_NONE && len > 0 {
            use x86_64::registers::control::Cr3;
            use x86_64::structures::paging::{OffsetPageTable, Page, PageTable, Size4KiB, Translate};
            use x86_64::VirtAddr;

            let hhdm = crate::memory::hhdm_offset();
            let (current_pml4, _) = Cr3::read();
            let pml4_virt = VirtAddr::new(hhdm + current_pml4.start_address().as_u64());
            let pml4_table: &mut PageTable = unsafe { &mut *pml4_virt.as_mut_ptr() };
            let mapper = unsafe { OffsetPageTable::new(pml4_table, VirtAddr::new(hhdm)) };

            let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(target_addr));
            let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(target_addr + len - 1));

            for page in Page::range_inclusive(start_page, end_page) {
                if let Some(phys) = mapper.translate_addr(page.start_address()) {
                    let frame_virt = hhdm + phys.as_u64();
                    let frame_slice = unsafe { core::slice::from_raw_parts_mut(frame_virt as *mut u8, 4096) };
                    let page_vstart = page.start_address().as_u64();
                    let page_file_off = offset + (page_vstart - target_addr);
                    let bytes_in_page = ((target_addr + len) - page_vstart).min(4096) as usize;
                    if bytes_in_page > 0 {
                        let _ = inode.read_at(page_file_off as usize, &mut frame_slice[..bytes_in_page]);
                    }
                }
            }
        }
    }

    target_addr
}


pub fn sys_munmap(addr: u64, len: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    match vma.munmap(addr, len) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    match vma.mprotect(addr, len, prot) {
        Ok(res) => res,
        Err(err) => (-(err.abs())) as u64,
    }
}


pub fn sys_brk(new_brk: u64) -> u64 {
    let mut vma = VMA_MANAGER.lock();
    vma.brk(new_brk)
}

pub fn reset_process_vma(brk_start: u64) {
    let mut vma = VMA_MANAGER.lock();
    vma.reset(brk_start);
}

pub fn is_valid_user_vma_addr(addr: u64) -> bool {
    let vma = VMA_MANAGER.lock();
    vma.is_valid_addr(addr)
}


pub fn sys_arch_prctl(code: u64, addr: u64) -> u64 {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            if addr >= USER_SPACE_END {
                return (-(EFAULT as i64)) as u64;
            }
            unsafe {
                crate::arch::x86_64::syscall::write_fs_base(addr);
            }
            crate::task::set_current_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            if !validate_user_buffer(addr, 8) {
                return (-(EFAULT as i64)) as u64;
            }
            let cur_fs = unsafe { crate::arch::x86_64::syscall::read_fs_base() };
            unsafe {
                core::ptr::write_unaligned(addr as *mut u64, cur_fs);
            }
            0
        }
        _ => (-(EINVAL as i64)) as u64,
    }
}

pub fn fill_random_bytes(slice: &mut [u8]) {
    crate::arch::x86_64::cpu::get_random_bytes(slice);
}

pub fn sys_mincore(addr: u64, len: u64, vec_ptr: u64) -> u64 {
    if (addr & 0xFFF) != 0 || len == 0 {
        return (-(EINVAL as i64)) as u64;
    }
    let pages = (len + 4095) / 4096;
    if !validate_user_buffer(vec_ptr, pages) {
        return (-(EFAULT as i64)) as u64;
    }
    match addr.checked_add(len) {
        Some(end) if end <= USER_SPACE_END => {}
        _ => return (-(super::ENOMEM as i64)) as u64,
    }

    let vec_slice = unsafe { core::slice::from_raw_parts_mut(vec_ptr as *mut u8, pages as usize) };
    for (i, byte) in vec_slice.iter_mut().enumerate() {
        let page_addr = addr + (i as u64) * 4096;
        let is_mapped = crate::memory::translate_address(page_addr).is_some()
            || is_valid_user_vma_addr(page_addr);
        *byte = if is_mapped { 1 } else { 0 };
    }
    0
}

pub fn sys_madvise(addr: u64, len: u64, advice: u64) -> u64 {
    if (addr & 0xFFF) != 0 {
        return (-(EINVAL as i64)) as u64;
    }
    match addr.checked_add(len) {
        Some(end) if end <= USER_SPACE_END => {}
        _ => return (-(EINVAL as i64)) as u64,
    }
    match advice {
        0 /* MADV_NORMAL */ | 1 /* MADV_RANDOM */ | 2 /* MADV_SEQUENTIAL */
        | 3 /* MADV_WILLNEED */ | 4 /* MADV_DONTNEED */ | 8 /* MADV_FREE */ => 0,
        _ => (-(EINVAL as i64)) as u64,
    }
}

pub fn sys_getrandom(buf_ptr: u64, len: u64, _flags: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return (-(EFAULT as i64)) as u64;
    }
    if len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    fill_random_bytes(slice);
    len
}
