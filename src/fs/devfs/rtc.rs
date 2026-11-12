use super::types::CharDeviceOp;
use crate::fs::vfs::VfsError;

pub const RTC_RD_TIME: u32 = 0x80247009;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxRtcTime {
    pub tm_sec: i32,
    pub tm_min: i32,
    pub tm_hour: i32,
    pub tm_mday: i32,
    pub tm_mon: i32,   // 0..11
    pub tm_year: i32,  // лет с 1900 года
    pub tm_wday: i32,
    pub tm_yday: i32,
    pub tm_isdst: i32,
}

/// /dev/rtc0: аппаратные часы реального времени CMOS MC146818
pub struct RtcDev;

impl CharDeviceOp for RtcDev {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let dt = crate::drivers::rtc::read_datetime();
        let formatted = alloc::format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC\n",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
        );
        let bytes = formatted.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let to_copy = buf.len().min(bytes.len() - offset);
        buf[..to_copy].copy_from_slice(&bytes[offset..offset + to_copy]);
        Ok(to_copy)
    }

    fn write(&self, _offset: usize, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::PermissionDenied)
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        match cmd {
            RTC_RD_TIME => {
                let sz = core::mem::size_of::<LinuxRtcTime>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let dt = crate::drivers::rtc::read_datetime();
                let rtc_time = LinuxRtcTime {
                    tm_sec: dt.second as i32,
                    tm_min: dt.minute as i32,
                    tm_hour: dt.hour as i32,
                    tm_mday: dt.day as i32,
                    tm_mon: (dt.month.saturating_sub(1)) as i32,
                    tm_year: (dt.year as i32) - 1900,
                    tm_wday: 0,
                    tm_yday: 0,
                    tm_isdst: 0,
                };
                unsafe {
                    core::ptr::write_unaligned(arg as *mut LinuxRtcTime, rtc_time);
                }
                Ok(0)
            }
            _ => Err(-25),
        }
    }
}
