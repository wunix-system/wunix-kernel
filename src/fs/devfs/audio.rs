use core::sync::atomic::{AtomicU32, Ordering};
use wunix_drv::abi::audio::*;

use super::types::CharDeviceOp;
use crate::fs::vfs::VfsError;

/// Символьный интерфейс `/dev/snd/pcmN` (`/dev/dsp`) для user-space аудио-сервера.
/// Предоставляет потоковый вывод PCM и `ioctl` управления параметрами поверх любого `AudioDevice`.
pub struct AudioPcmDev {
    index: usize,
    sample_rate: AtomicU32,
    channels: AtomicU32,
    format: AtomicU32,
    volume: AtomicU32,
}

impl AudioPcmDev {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            sample_rate: AtomicU32::new(48_000),
            channels: AtomicU32::new(2),
            format: AtomicU32::new(AFMT_S16_LE),
            volume: AtomicU32::new(80),
        }
    }
}

impl CharDeviceOp for AudioPcmDev {
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotSupported)
    }

    fn write(&self, _offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let dev = crate::drivers::get_audio_device(self.index).ok_or(VfsError::NotFound)?;
        let mut guard = dev.lock();
        guard.play_pcm(buf).map_err(|_| VfsError::IoError)?;
        Ok(buf.len())
    }

    fn poll_events(&self) -> u32 {
        let Some(dev) = crate::drivers::get_audio_device(self.index) else {
            return crate::fs::EPOLLERR;
        };
        if dev.lock().is_busy() {
            0
        } else {
            crate::fs::EPOLLOUT
        }
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        let dev = crate::drivers::get_audio_device(self.index).ok_or(-19i64)?;
        match cmd {
            SNDCTL_DSP_RESET | SNDCTL_DSP_SYNC => Ok(0),
            SNDCTL_DSP_SPEED => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let rate = unsafe { core::ptr::read_unaligned(arg as *const u32) };
                let clamped = rate.clamp(8_000, 192_000);
                self.sample_rate.store(clamped, Ordering::Release);
                unsafe { core::ptr::write_unaligned(arg as *mut u32, clamped); }
                Ok(0)
            }
            SNDCTL_DSP_CHANNELS => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let ch = unsafe { core::ptr::read_unaligned(arg as *const u32) }.clamp(1, 2);
                self.channels.store(ch, Ordering::Release);
                unsafe { core::ptr::write_unaligned(arg as *mut u32, ch); }
                Ok(0)
            }
            SNDCTL_DSP_STEREO => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let st = unsafe { core::ptr::read_unaligned(arg as *const u32) };
                let ch = if st != 0 { 2 } else { 1 };
                self.channels.store(ch, Ordering::Release);
                unsafe { core::ptr::write_unaligned(arg as *mut u32, if ch == 2 { 1 } else { 0 }); }
                Ok(0)
            }
            SNDCTL_DSP_SETFMT => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let fmt = unsafe { core::ptr::read_unaligned(arg as *const u32) };
                let valid = match fmt {
                    AFMT_U8 | AFMT_S16_LE | AFMT_S32_LE => fmt,
                    _ => AFMT_S16_LE,
                };
                self.format.store(valid, Ordering::Release);
                unsafe { core::ptr::write_unaligned(arg as *mut u32, valid); }
                Ok(0)
            }
            SNDCTL_DSP_GETCAPS => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                unsafe {
                    core::ptr::write_unaligned(arg as *mut u32, DSP_CAP_BATCH | DSP_CAP_TRIGGER);
                }
                Ok(0)
            }
            SNDCTL_DSP_GETOSPACE => {
                let sz = core::mem::size_of::<AudioBufInfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let busy = dev.lock().is_busy();
                let info = AudioBufInfo {
                    fragments: if busy { 0 } else { 4 },
                    fragstotal: 4,
                    fragsize: 4096,
                    bytes: if busy { 0 } else { 16384 },
                };
                unsafe { core::ptr::write_unaligned(arg as *mut AudioBufInfo, info); }
                Ok(0)
            }
            SNDCTL_DSP_SETVOLUME => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let vol = unsafe { core::ptr::read_unaligned(arg as *const u32) }.min(100) as u8;
                dev.lock().set_volume(vol).map_err(|_| -5i64)?;
                self.volume.store(vol as u32, Ordering::Release);
                Ok(0)
            }
            SNDCTL_DSP_GETVOLUME => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                unsafe {
                    core::ptr::write_unaligned(arg as *mut u32, self.volume.load(Ordering::Acquire));
                }
                Ok(0)
            }
            SNDCTL_DSP_GETINFO => {
                let sz = core::mem::size_of::<AudioDeviceInfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let drv_name = dev.lock().device_name();
                let mut name = [0u8; 24];
                let n = drv_name.len().min(23);
                name[..n].copy_from_slice(&drv_name.as_bytes()[..n]);
                let info = AudioDeviceInfo {
                    sample_rate: self.sample_rate.load(Ordering::Acquire),
                    channels: self.channels.load(Ordering::Acquire),
                    format: self.format.load(Ordering::Acquire),
                    volume: self.volume.load(Ordering::Acquire),
                    capabilities: DSP_CAP_BATCH | DSP_CAP_TRIGGER,
                    buffer_bytes: 16384,
                    name,
                };
                unsafe { core::ptr::write_unaligned(arg as *mut AudioDeviceInfo, info); }
                Ok(0)
            }
            _ => Err(-25),
        }
    }
}
