use core::arch::x86_64::{__cpuid, _rdtsc};
use core::sync::atomic::{fence, Ordering};
use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::PhysFrame;

use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

const MSR_KVM_WALL_CLOCK_NEW: u32 = 0x4B56_4D00;
const MSR_KVM_SYSTEM_TIME_NEW: u32 = 0x4B56_4D01;

const KVM_FEATURE_CLOCKSOURCE: u32 = 1 << 0;
const KVM_FEATURE_CLOCKSOURCE2: u32 = 1 << 3;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct KvmWallClock {
    pub version: u32,
    pub sec: u32,
    pub nsec: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PvclockVcpuTimeInfo {
    pub version: u32,
    pub pad0: u32,
    pub tsc_timestamp: u64,
    pub system_time: u64,
    pub tsc_to_system_mul: u32,
    pub tsc_shift: i8,
    pub flags: u8,
    pub pad: [u8; 2],
}

fn has_kvm_clock_cpuid() -> bool {
    let base = __cpuid(0x4000_0000);
    let mut sig = [0u8; 12];
    sig[0..4].copy_from_slice(&base.ebx.to_le_bytes());
    sig[4..8].copy_from_slice(&base.ecx.to_le_bytes());
    sig[8..12].copy_from_slice(&base.edx.to_le_bytes());
    if &sig != b"KVMKVMKVM\0\0\0" {
        return false;
    }
    let feat = __cpuid(0x4000_0001);
    (feat.eax & (KVM_FEATURE_CLOCKSOURCE | KVM_FEATURE_CLOCKSOURCE2)) != 0
}

/// Клиент высокоточных часов KVM pvclock для гостевых систем Linux/KVM и Proxmox VE.
pub struct KvmPvClock {
    wall_frame: PhysFrame,
    wall_ptr: *mut KvmWallClock,
    sys_frame: PhysFrame,
    sys_ptr: *mut PvclockVcpuTimeInfo,
}

unsafe impl Send for KvmPvClock {}
unsafe impl Sync for KvmPvClock {}

impl KvmPvClock {
    pub fn new() -> Result<Self, &'static str> {
        if !has_kvm_clock_cpuid() {
            return Err("pvclock: KVM clocksource feature not present in CPUID");
        }

        let wall_frame =
            allocate_frame().ok_or("pvclock: failed to allocate wallclock frame")?;
        let wall_phys = wall_frame.start_address().as_u64();
        let wall_ptr = (hhdm_offset() + wall_phys) as *mut KvmWallClock;

        let sys_frame = match allocate_frame() {
            Some(f) => f,
            None => {
                deallocate_frame(wall_frame);
                return Err("pvclock: failed to allocate system_time frame");
            }
        };
        let sys_phys = sys_frame.start_address().as_u64();
        let sys_ptr = (hhdm_offset() + sys_phys) as *mut PvclockVcpuTimeInfo;

        unsafe {
            core::ptr::write_bytes(wall_ptr as *mut u8, 0, 4096);
            core::ptr::write_bytes(sys_ptr as *mut u8, 0, 4096);
            Msr::new(MSR_KVM_WALL_CLOCK_NEW).write(wall_phys);
            Msr::new(MSR_KVM_SYSTEM_TIME_NEW).write(sys_phys | 1);
        }

        Ok(Self {
            wall_frame,
            wall_ptr,
            sys_frame,
            sys_ptr,
        })
    }

    pub fn monotonic_ns(&self) -> Option<u64> {
        unsafe {
            for _ in 0..16 {
                let v1 = core::ptr::read_volatile(&(*self.sys_ptr).version);
                if (v1 & 1) != 0 {
                    core::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let tsc_base = core::ptr::read_volatile(&(*self.sys_ptr).tsc_timestamp);
                let sys_time = core::ptr::read_volatile(&(*self.sys_ptr).system_time);
                let mul = core::ptr::read_volatile(&(*self.sys_ptr).tsc_to_system_mul);
                let shift = core::ptr::read_volatile(&(*self.sys_ptr).tsc_shift);
                let now_tsc = _rdtsc();
                fence(Ordering::Acquire);
                let v2 = core::ptr::read_volatile(&(*self.sys_ptr).version);
                if v1 == v2 && v1 != 0 {
                    let mut delta = now_tsc.saturating_sub(tsc_base);
                    if shift < 0 {
                        delta >>= (-shift) as u32;
                    } else if shift > 0 {
                        delta <<= shift as u32;
                    }
                    let ns = ((delta as u128 * mul as u128) >> 32) as u64;
                    return Some(sys_time.saturating_add(ns));
                }
            }
        }
        None
    }

    pub fn read_time(&self) -> Option<(u64, u32)> {
        unsafe {
            for _ in 0..16 {
                let v1 = core::ptr::read_volatile(&(*self.wall_ptr).version);
                if (v1 & 1) != 0 {
                    core::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let base_sec = core::ptr::read_volatile(&(*self.wall_ptr).sec) as u64;
                let base_nsec = core::ptr::read_volatile(&(*self.wall_ptr).nsec);
                fence(Ordering::Acquire);
                let v2 = core::ptr::read_volatile(&(*self.wall_ptr).version);
                if v1 == v2 && base_sec > 0 {
                    let total_ns = base_sec * 1_000_000_000
                        + base_nsec as u64
                        + self.monotonic_ns().unwrap_or(0);
                    return Some((total_ns / 1_000_000_000, (total_ns % 1_000_000_000) as u32));
                }
            }
        }
        None
    }
}

pub fn is_present() -> bool {
    super::KVM_PVCLOCK.lock().is_some()
}

pub fn read_wall_clock() -> Option<(u64, u32)> {
    super::KVM_PVCLOCK
        .lock()
        .as_ref()
        .and_then(|pv| pv.read_time())
        .filter(|(sec, _)| *sec > 0)
}

impl Drop for KvmPvClock {
    fn drop(&mut self) {
        unsafe {
            Msr::new(MSR_KVM_SYSTEM_TIME_NEW).write(0);
            Msr::new(MSR_KVM_WALL_CLOCK_NEW).write(0);
        }
        deallocate_frame(self.sys_frame);
        deallocate_frame(self.wall_frame);
    }
}
