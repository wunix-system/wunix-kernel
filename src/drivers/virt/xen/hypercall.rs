use core::arch::x86_64::__cpuid;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

pub const HYPERVISOR_SET_TRAP_TABLE: usize = 0;
pub const HYPERVISOR_MMU_UPDATE: usize = 1;
pub const HYPERVISOR_SCHED_OP: usize = 6;
pub const HYPERVISOR_MEMORY_OP: usize = 12;
pub const HYPERVISOR_EVENT_CHANNEL_OP: usize = 16;
pub const HYPERVISOR_XEN_VERSION: usize = 17;
pub const HYPERVISOR_CONSOLE_IO: usize = 18;
pub const HYPERVISOR_GRANT_TABLE_OP: usize = 20;

pub const CONSOLEIO_WRITE: usize = 0;
pub const CONSOLEIO_READ: usize = 1;

pub const XENVER_VERSION: usize = 0;
pub const XENVER_EXTRAVERSION: usize = 1;

/// Страница шлюза гипервызовов Xen (Hypercall Page).
pub struct XenHypercallPage {
    frame: PhysFrame,
    virt_addr: *mut u8,
}

unsafe impl Send for XenHypercallPage {}
unsafe impl Sync for XenHypercallPage {}

impl XenHypercallPage {
    pub fn new() -> Result<Self, &'static str> {
        let leaf = __cpuid(0x4000_0002);
        let msr_base = leaf.ebx;
        if msr_base == 0 {
            return Err("Xen: Hypercall MSR not available");
        }

        let frame = allocate_frame().ok_or("Xen: failed to allocate hypercall page")?;
        let phys_addr = frame.start_address().as_u64();
        let virt_addr = (hhdm_offset() + phys_addr) as *mut u8;

        unsafe {
            core::ptr::write_bytes(virt_addr, 0x90, 4096);
            let mut msr = x86_64::registers::model_specific::Msr::new(msr_base);
            msr.write(phys_addr);
        }

        Ok(Self { frame, virt_addr })
    }

    pub unsafe fn call(&self, op: usize, arg1: usize, arg2: usize, arg3: usize) -> isize {
        let ret: isize;
        unsafe {
            let fn_ptr = self.virt_addr.add(op * 32);
            core::arch::asm!(
                "call {fn_target}",
                fn_target = in(reg) fn_ptr,
                in("rdi") arg1,
                in("rsi") arg2,
                in("rdx") arg3,
                lateout("rax") ret,
                clobber_abi("C"),
            );
        }
        ret
    }
}

impl Drop for XenHypercallPage {
    fn drop(&mut self) {
        deallocate_frame(self.frame);
    }
}

pub static XEN_HYPERCALL: Mutex<Option<XenHypercallPage>> = Mutex::new(None);

pub fn hypercall3(op: usize, arg1: usize, arg2: usize, arg3: usize) -> isize {
    let guard = XEN_HYPERCALL.lock();
    if let Some(page) = guard.as_ref() {
        unsafe { page.call(op, arg1, arg2, arg3) }
    } else {
        -1
    }
}

pub fn xen_version() -> Option<(u16, u16)> {
    let ret = hypercall3(HYPERVISOR_XEN_VERSION, XENVER_VERSION, 0, 0);
    if ret >= 0 {
        let major = ((ret >> 16) & 0xFFFF) as u16;
        let minor = (ret & 0xFFFF) as u16;
        Some((major, minor))
    } else {
        None
    }
}

pub fn xen_console_write(msg: &str) {
    let bytes = msg.as_bytes();
    hypercall3(HYPERVISOR_CONSOLE_IO, CONSOLEIO_WRITE, bytes.len(), bytes.as_ptr() as usize);
}
