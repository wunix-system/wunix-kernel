use core::sync::atomic::{fence, Ordering};
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use super::hypercall::{hypercall3, HYPERVISOR_GRANT_TABLE_OP};
use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

pub const GTF_INVALID: u16 = 0;
pub const GTF_PERMIT_ACCESS: u16 = 1;
pub const GTF_READONLY: u16 = 4;

pub const GNTTABOP_SETUP_TABLE: usize = 0;

/// Элемент таблицы прав доступа Xen Grant Table v1 (8 байт).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GrantEntryV1 {
    pub flags: u16,
    pub domid: u16,
    pub frame: u32,
}

#[repr(C)]
struct GnttabSetupTable {
    dom: u16,
    nr_frames: u32,
    status: i16,
    frame_list: u64,
}

/// Таблица прав доступа к страницам физической памяти Xen Grant Table.
pub struct GrantTable {
    frame: PhysFrame,
    entries: *mut GrantEntryV1,
    num_entries: usize,
}

unsafe impl Send for GrantTable {}
unsafe impl Sync for GrantTable {}

impl GrantTable {
    pub fn new() -> Result<Self, &'static str> {
        let frame = allocate_frame().ok_or("Xen: failed to allocate grant table frame")?;
        let phys_addr = frame.start_address().as_u64();
        let virt_addr = (hhdm_offset() + phys_addr) as *mut GrantEntryV1;

        unsafe {
            core::ptr::write_bytes(virt_addr as *mut u8, 0, 4096);
        }

        let num_entries = 4096 / core::mem::size_of::<GrantEntryV1>();

        let mut pfn = (phys_addr / 4096) as u64;
        let setup = GnttabSetupTable {
            dom: 0x7FF0,
            nr_frames: 1,
            status: 0,
            frame_list: &mut pfn as *mut _ as u64,
        };

        let _ = hypercall3(
            HYPERVISOR_GRANT_TABLE_OP,
            GNTTABOP_SETUP_TABLE,
            &setup as *const _ as usize,
            1,
        );

        Ok(Self {
            frame,
            entries: virt_addr,
            num_entries,
        })
    }

    pub fn grant_access(&mut self, ref_idx: usize, frame_pfn: u32, readonly: bool) -> Result<(), &'static str> {
        if ref_idx >= self.num_entries {
            return Err("Xen grant ref out of bounds");
        }

        let mut flags = GTF_PERMIT_ACCESS;
        if readonly {
            flags |= GTF_READONLY;
        }

        unsafe {
            let entry = &mut *self.entries.add(ref_idx);
            entry.domid = 0;
            entry.frame = frame_pfn;
            fence(Ordering::SeqCst);
            entry.flags = flags;
        }

        Ok(())
    }

    pub fn end_access(&mut self, ref_idx: usize) {
        if ref_idx < self.num_entries {
            unsafe {
                let entry = &mut *self.entries.add(ref_idx);
                entry.flags = GTF_INVALID;
                fence(Ordering::SeqCst);
            }
        }
    }
}

impl Drop for GrantTable {
    fn drop(&mut self) {
        deallocate_frame(self.frame);
    }
}

pub static GRANT_TABLE: Mutex<Option<GrantTable>> = Mutex::new(None);
