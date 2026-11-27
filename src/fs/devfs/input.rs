use alloc::collections::VecDeque;
use alloc::format;
use alloc::sync::Arc;
use spin::Mutex;
use wunix_drv::abi::display::InputEventRecord;

use super::types::{CharDeviceOp, INPUT_MAJOR};
use crate::fs::vfs::VfsError;

static MOUSE_PACKET_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

pub fn push_mouse_packet(pkt: [u8; 3]) {
    let mut q = MOUSE_PACKET_QUEUE.lock();
    while q.len() + 3 > 256 {
        q.pop_front();
    }
    q.push_back(pkt[0]);
    q.push_back(pkt[1]);
    q.push_back(pkt[2]);
}

/// /dev/input/mice: поток 3-байтных пакетов мыши (PS/2 Mouse Protocol) без фиктивных пакетов покоя.
pub struct MiceDev;

impl CharDeviceOp for MiceDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut q = MOUSE_PACKET_QUEUE.lock();
        if q.is_empty() {
            return Ok(0);
        }
        let to_copy = buf.len().min(q.len());
        for dst in buf[..to_copy].iter_mut() {
            *dst = q.pop_front().unwrap();
        }
        Ok(to_copy)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let mut ev = crate::fs::EPOLLOUT;
        if !MOUSE_PACKET_QUEUE.lock().is_empty() {
            ev |= crate::fs::EPOLLIN;
        }
        ev
    }
}

/// /dev/input/eventN: универсальный поток событий подсистемы evdev для указанного устройства ввода.
pub struct EventDev {
    pub index: usize,
    pending: Mutex<Option<crate::drivers::InputEvent>>,
}

impl EventDev {
    pub const fn new(index: usize) -> Self {
        Self {
            index,
            pending: Mutex::new(None),
        }
    }

    fn fetch_next_event(&self) -> Option<crate::drivers::InputEvent> {
        if let Some(ev) = self.pending.lock().take() {
            return Some(ev);
        }
        crate::drivers::with_device_manager(|dm| {
            if let Some(dev) = dm.get_input_device(self.index) {
                return dev.lock().poll_event();
            }
            None
        })
        .unwrap_or(None)
    }
}

impl CharDeviceOp for EventDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let ev_size = core::mem::size_of::<InputEventRecord>();
        if buf.len() < ev_size {
            return Ok(0);
        }

        let mut read_bytes = 0usize;
        while read_bytes + ev_size <= buf.len() {
            let Some(ev) = self.fetch_next_event() else {
                break;
            };
            let ticks = crate::arch::x86_64::timer::ticks();
            let lev = InputEventRecord {
                time_sec: ticks / 100,
                time_usec: (ticks % 100) * 10_000,
                type_: ev.type_,
                code: ev.code,
                value: ev.value as i32,
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &lev as *const InputEventRecord as *const u8,
                    buf.as_mut_ptr().add(read_bytes),
                    ev_size,
                );
            }
            read_bytes += ev_size;
        }

        Ok(read_bytes)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let mut p = self.pending.lock();
        if p.is_none() {
            *p = crate::drivers::with_device_manager(|dm| {
                dm.get_input_device(self.index)?.lock().poll_event()
            })
            .unwrap_or(None);
        }
        if p.is_some() {
            crate::fs::EPOLLIN | crate::fs::EPOLLOUT
        } else {
            crate::fs::EPOLLOUT
        }
    }
}

/// Экспонирует в `/dev/input/eventN` все устройства ввода, зарегистрированные в `DeviceManager`.
pub fn sync_input_nodes() {
    let Some(devfs) = super::get_devfs() else {
        return;
    };
    let count = crate::drivers::with_device_manager(|dm| dm.input_device_count()).unwrap_or(0);
    for idx in 0..count {
        let path = format!("input/event{}", idx);
        devfs.register_char(&path, INPUT_MAJOR, 64 + idx as u32, Arc::new(EventDev::new(idx)), 0o660);
    }
}
