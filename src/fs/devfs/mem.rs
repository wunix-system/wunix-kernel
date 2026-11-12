use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::port::Port;

use super::types::CharDeviceOp;
use crate::fs::vfs::VfsError;

/// /dev/null: битовая корзина системы (read возвращает 0/EOF, write подтверждает прием)
pub struct NullDev;

impl CharDeviceOp for NullDev {
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Ok(0)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }
}

/// /dev/zero: неисчерпаемый источник нулевых байтов
pub struct ZeroDev;

impl CharDeviceOp for ZeroDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }
}

/// /dev/full: устройство для тестирования переполнения (запись всегда отклоняется с ENOSPC)
pub struct FullDev;

impl CharDeviceOp for FullDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write(&self, _offset: usize, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::IoError) // Системный вызов транслирует это в -ENOSPC (-28)
    }
}

/// /dev/random и /dev/urandom: криптографический и псевдослучайный генератор ядра
pub struct RandomDev;

impl CharDeviceOp for RandomDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        crate::task::syscall::mem::fill_random_bytes(buf);
        Ok(buf.len())
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }
}

/// /dev/mem: прямой доступ к физической памяти через HHDM (Higher-Half Direct Map)
pub struct MemDev;

impl CharDeviceOp for MemDev {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let hhdm = crate::memory::hhdm_offset();
        let target_virt = hhdm.wrapping_add(offset as u64);
        unsafe {
            core::ptr::copy_nonoverlapping(target_virt as *const u8, buf.as_mut_ptr(), buf.len());
        }
        Ok(buf.len())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let hhdm = crate::memory::hhdm_offset();
        let target_virt = hhdm.wrapping_add(offset as u64);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), target_virt as *mut u8, buf.len());
        }
        Ok(buf.len())
    }
}

/// /dev/kmem: виртуальная память адресного пространства ядра
pub struct KmemDev;

impl CharDeviceOp for KmemDev {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        if offset < 0xFFFF_8000_0000_0000 {
            return Err(VfsError::PermissionDenied);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(offset as *const u8, buf.as_mut_ptr(), buf.len());
        }
        Ok(buf.len())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        if offset < 0xFFFF_8000_0000_0000 {
            return Err(VfsError::PermissionDenied);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), offset as *mut u8, buf.len());
        }
        Ok(buf.len())
    }
}

/// /dev/port: прямой доступ к портам ввода-вывода x86 (I/O Port Space 0x0000-0xFFFF)
pub struct PortDev;

impl CharDeviceOp for PortDev {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        for (i, b) in buf.iter_mut().enumerate() {
            let p = offset + i;
            if p > 0xFFFF {
                break;
            }
            let mut port = Port::<u8>::new(p as u16);
            *b = unsafe { port.read() };
        }
        Ok(buf.len())
    }

    fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        for (i, &b) in buf.iter().enumerate() {
            let p = offset + i;
            if p > 0xFFFF {
                break;
            }
            let mut port = Port::<u8>::new(p as u16);
            unsafe { port.write(b); }
        }
        Ok(buf.len())
    }
}

static KMSG_LOG_QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Добавляет строку в общесистемный буфер /dev/kmsg
pub fn push_kmsg(line: &str) {
    let mut q = KMSG_LOG_QUEUE.lock();
    if q.len() > 1024 {
        q.remove(0);
    }
    q.push(String::from(line));
}

/// /dev/kmsg: интерфейс кольцевого буфера сообщений ядра Linux printk
pub struct KmsgDev;

impl CharDeviceOp for KmsgDev {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let q = KMSG_LOG_QUEUE.lock();
        let mut full_text = String::new();
        for msg in q.iter() {
            full_text.push_str("6,0,0,-;"); // facility 0 (kernel), level 6 (info)
            full_text.push_str(msg);
            if !msg.ends_with('\n') {
                full_text.push('\n');
            }
        }
        let bytes = full_text.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let available = &bytes[offset..];
        let to_copy = buf.len().min(available.len());
        buf[..to_copy].copy_from_slice(&available[..to_copy]);
        Ok(to_copy)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if let Ok(s) = core::str::from_utf8(buf) {
            crate::klog!("{}", s);
            push_kmsg(s);
        }
        Ok(buf.len())
    }
}
