use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::PhysFrame;

use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

// Спецификация KVM Paravirtual Clock: MSR регистрации буфера системного времени хоста
const MSR_KVM_WALL_CLOCK_NEW: u32 = 0x4B56_4D00;

/// Структура структуры времени хоста KVM pvclock (RFC kvm paravirt).
/// Гипервизор атомарно обновляет секунды и наносекунды, инкрементируя четную версию.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct KvmWallClock {
    pub version: u32,
    pub sec: u32,
    pub nsec: u32,
}

/// Клиент высокоточных часов KVM pvclock для гостевых систем Linux/KVM и Proxmox VE.
/// Позволяет мгновенно получать время хоста без дрейфа таймера PIT и без оверхеда переключения контекста VM-exit.
pub struct KvmPvClock {
    frame: PhysFrame,
    virt_ptr: *mut KvmWallClock,
}

unsafe impl Send for KvmPvClock {}
unsafe impl Sync for KvmPvClock {}

impl KvmPvClock {
    pub fn new() -> Result<Self, &'static str> {
        let frame = allocate_frame().ok_or("pvclock: failed to allocate wallclock frame")?;
        let phys_addr = frame.start_address().as_u64();
        let virt_ptr = (hhdm_offset() + phys_addr) as *mut KvmWallClock;

        unsafe {
            core::ptr::write_bytes(virt_ptr as *mut u8, 0, 4096);
            // Регистрируем физический адрес страницы времени в гипервизоре
            let mut msr = Msr::new(MSR_KVM_WALL_CLOCK_NEW);
            msr.write(phys_addr);
        }

        Ok(Self { frame, virt_ptr })
    }

    /// Считывает текущее время UTC хоста (секунды и наносекунды) с проверкой версионного спинлока.
    pub fn read_time(&self) -> Option<(u64, u32)> {
        unsafe {
            let clock = &*self.virt_ptr;
            for _ in 0..10 {
                let v1 = core::ptr::read_volatile(&clock.version);
                if v1 & 1 != 0 {
                    // Нечетная версия означает, что хост прямо сейчас производит запись
                    core::hint::spin_loop();
                    continue;
                }
                let sec = core::ptr::read_volatile(&clock.sec) as u64;
                let nsec = core::ptr::read_volatile(&clock.nsec);
                let v2 = core::ptr::read_volatile(&clock.version);
                if v1 == v2 {
                    return Some((sec, nsec));
                }
            }
        }
        None
    }
}

impl Drop for KvmPvClock {
    fn drop(&mut self) {
        unsafe {
            let mut msr = Msr::new(MSR_KVM_WALL_CLOCK_NEW);
            msr.write(0);
        }
        deallocate_frame(self.frame);
    }
}
