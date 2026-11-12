use spin::Mutex;

use super::types::CharDeviceOp;
use crate::fs::vfs::VfsError;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxInputEvent {
    pub time_sec: u64,
    pub time_usec: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

static MOUSE_PACKET_QUEUE: Mutex<[u8; 128]> = Mutex::new([0u8; 128]);
static MOUSE_QUEUE_LEN: Mutex<usize> = Mutex::new(0);

pub fn push_mouse_packet(pkt: [u8; 3]) {
    let mut q = MOUSE_PACKET_QUEUE.lock();
    let mut len = MOUSE_QUEUE_LEN.lock();
    if *len + 3 <= 128 {
        q[*len] = pkt[0];
        q[*len + 1] = pkt[1];
        q[*len + 2] = pkt[2];
        *len += 3;
    }
}

/// /dev/input/mice: поток пакетов мыши по 3 байта (PS/2 Mouse Protocol)
pub struct MiceDev;

impl CharDeviceOp for MiceDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut q = MOUSE_PACKET_QUEUE.lock();
        let mut len = MOUSE_QUEUE_LEN.lock();

        if *len == 0 {
            // Если очередь пуста, возвращаем нейтральный пакет покоя [0x08, 0, 0]
            if buf.len() >= 3 {
                buf[0] = 0x08; // Синхробит PS/2 мыши
                buf[1] = 0;
                buf[2] = 0;
                return Ok(3);
            }
            return Ok(0);
        }

        let to_copy = buf.len().min(*len);
        buf[..to_copy].copy_from_slice(&q[..to_copy]);
        q.copy_within(to_copy..*len, 0);
        *len -= to_copy;
        Ok(to_copy)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }
}

/// /dev/input/event0: универсальный поток событий подсистемы Linux evdev
pub struct EventDev;

impl CharDeviceOp for EventDev {
    fn read(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let ev_size = core::mem::size_of::<LinuxInputEvent>();
        if buf.len() < ev_size {
            return Ok(0);
        }

        // Опрашиваем активные устройства ввода ядра (VirtIO Input, VBox Tablet, etc.)
        let event_opt = crate::drivers::with_device_manager(|dm| {
            for i in 0..dm.input_device_count() {
                if let Some(dev) = dm.get_input_device(i) {
                    if let Some(ev) = dev.lock().poll_event() {
                        return Some(ev);
                    }
                }
            }
            None
        }).unwrap_or(None);

        if let Some(ev) = event_opt {
            let ticks = crate::arch::x86_64::timer::ticks();
            let lev = LinuxInputEvent {
                time_sec: ticks / 100,
                time_usec: (ticks % 100) * 10_000,
                type_: ev.type_,
                code: ev.code,
                value: ev.value as i32,
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &lev as *const LinuxInputEvent as *const u8,
                    buf.as_mut_ptr(),
                    ev_size,
                );
            }
            Ok(ev_size)
        } else {
            Ok(0)
        }
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        Ok(buf.len())
    }
}
