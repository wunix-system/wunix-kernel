use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;
use wunix_drv::abi::uio::{UioDeviceInfo, UIO_IOCTL_DISABLE_IRQ, UIO_IOCTL_ENABLE_IRQ, UIO_IOCTL_GET_INFO};

use super::types::{CharDeviceOp, UIO_MAJOR};
use crate::fs::vfs::{MmapBacking, VfsError};

/// Аппаратный дескриптор устройства UIO (Userspace I/O) для безопасного маппинга MMIO и ожидания IRQ.
pub struct UioDeviceNode {
    name: [u8; 16],
    mem_phys: u64,
    mem_size: u64,
    irq: u32,
    irq_count: AtomicU32,
    irq_pending: AtomicBool,
    irq_enabled: AtomicBool,
}

impl UioDeviceNode {
    pub fn new(name_str: &str, mem_phys: u64, mem_size: u64, irq: u32) -> Self {
        let mut name = [0u8; 16];
        let bytes = name_str.as_bytes();
        let copy_len = bytes.len().min(15);
        name[..copy_len].copy_from_slice(&bytes[..copy_len]);
        Self {
            name,
            mem_phys,
            mem_size,
            irq,
            irq_count: AtomicU32::new(0),
            irq_pending: AtomicBool::new(false),
            irq_enabled: AtomicBool::new(true),
        }
    }

    pub fn signal_irq(&self) {
        if self.irq_enabled.load(Ordering::Relaxed) {
            self.irq_count.fetch_add(1, Ordering::Release);
            self.irq_pending.store(true, Ordering::Release);
        }
    }
}

impl CharDeviceOp for UioDeviceNode {
    fn size(&self) -> usize {
        self.mem_size as usize
    }

    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.len() < 4 {
            return Err(VfsError::InvalidPath);
        }
        if !self.irq_pending.swap(false, Ordering::AcqRel) {
            return Ok(0);
        }
        let cnt = self.irq_count.load(Ordering::Acquire);
        buf[..4].copy_from_slice(&cnt.to_le_bytes());
        Ok(4)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.len() >= 4 {
            let val = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            self.irq_enabled.store(val != 0, Ordering::Release);
        }
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let mut ev = crate::fs::EPOLLOUT;
        if self.irq_pending.load(Ordering::Acquire) {
            ev |= crate::fs::EPOLLIN;
        }
        ev
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        match cmd {
            UIO_IOCTL_GET_INFO => {
                let sz = core::mem::size_of::<UioDeviceInfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let info = UioDeviceInfo {
                    name: self.name,
                    mem_phys: self.mem_phys,
                    mem_size: self.mem_size,
                    irq: self.irq,
                    irq_count: self.irq_count.load(Ordering::Acquire),
                };
                unsafe {
                    core::ptr::write_unaligned(arg as *mut UioDeviceInfo, info);
                }
                Ok(0)
            }
            UIO_IOCTL_ENABLE_IRQ => {
                self.irq_enabled.store(true, Ordering::Release);
                Ok(0)
            }
            UIO_IOCTL_DISABLE_IRQ => {
                self.irq_enabled.store(false, Ordering::Release);
                Ok(0)
            }
            _ => Err(-25),
        }
    }

    fn mmap_backing(&self, offset: usize, len: usize) -> Option<MmapBacking> {
        if self.mem_phys == 0 || (self.mem_phys & 0xFFF) != 0 {
            return None;
        }
        let end = offset.checked_add(len)?;
        if end as u64 > ((self.mem_size + 4095) & !4095) {
            return None;
        }
        Some(MmapBacking::PhysRange {
            phys_base: self.mem_phys + offset as u64,
            byte_len: len,
            uncacheable: true,
        })
    }
}

static UIO_DEVICES: Mutex<Vec<Arc<UioDeviceNode>>> = Mutex::new(Vec::new());

/// Регистрирует новое UIO-устройство в `/dev/uioN` (вызывается драйверами ядра или SDK).
pub fn register_uio_device(name: &str, mem_phys: u64, mem_size: u64, irq: u32) -> usize {
    let node = Arc::new(UioDeviceNode::new(name, mem_phys, mem_size, irq));
    let mut list = UIO_DEVICES.lock();
    let idx = list.len();
    list.push(node.clone());
    drop(list);

    if let Some(devfs) = super::get_devfs() {
        let dev_name = format!("uio{}", idx);
        devfs.register_char(&dev_name, UIO_MAJOR, idx as u32, node, 0o660);
    }
    idx
}

pub fn signal_uio_irq(idx: usize) {
    if let Some(dev) = UIO_DEVICES.lock().get(idx) {
        dev.signal_irq();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_uio_device_irq_counter_and_mmap() {
        let node = UioDeviceNode::new("test_uio", 0xFE00_0000, 4096, 11);
        node.signal_irq();
        node.signal_irq();
        assert_ne!(node.poll_events() & crate::fs::EPOLLIN, 0);
        let mut buf = [0u8; 4];
        let n = node.read(0, &mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(u32::from_le_bytes(buf), 2);
        assert_eq!(node.poll_events() & crate::fs::EPOLLIN, 0);
        assert!(node.mmap_backing(0, 4096).is_some());
    }
}
