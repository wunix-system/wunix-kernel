use crate::fs::vfs::VfsError;

/// Формирует 64-битный идентификатор устройства dev_t (Linux ABI)
pub const fn makedev(major: u32, minor: u32) -> u64 {
    (((major & 0xfffff000) as u64) << 32)
        | (((major & 0x00000fff) as u64) << 8)
        | (((minor & 0xffffff00) as u64) << 12)
        | ((minor & 0x000000ff) as u64)
}

pub const fn major(dev: u64) -> u32 {
    (((dev >> 32) & 0xfffff000) | ((dev >> 8) & 0x00000fff)) as u32
}

pub const fn minor(dev: u64) -> u32 {
    (((dev >> 12) & 0xffffff00) | (dev & 0x000000ff)) as u32
}

// Стандартные номера старших устройств Linux (Linux Device Numbers)
pub const MEM_MAJOR: u32 = 1;
pub const TTY_MAJOR: u32 = 4;
pub const TTYAUX_MAJOR: u32 = 5;
pub const LOOP_MAJOR: u32 = 7;
pub const SCSI_DISK_MAJOR: u32 = 8;
pub const MISC_MAJOR: u32 = 10;
pub const INPUT_MAJOR: u32 = 13;
pub const FB_MAJOR: u32 = 29;
pub const RTC_MAJOR: u32 = 254;
pub const VIRTBLK_MAJOR: u32 = 254;

/// Контракт драйвера символьного устройства ядра Wunix.
/// Символьные устройства предоставляют потоковый ввод-вывод, произвольный доступ
/// по смещению (для mem/fb) и специализированные ioctl-команды.
pub trait CharDeviceOp: Send + Sync {
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError>;
    fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError>;

    fn ioctl(&self, _cmd: u32, _arg: u64) -> Result<u64, i64> {
        Err(-25) // -ENOTTY
    }

    fn poll_events(&self) -> u32 {
        crate::fs::EPOLLIN | crate::fs::EPOLLOUT
    }

    fn is_tty(&self) -> bool {
        false
    }

    fn size(&self) -> usize {
        0
    }
}
