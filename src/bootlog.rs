use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

static HEAP_READY: AtomicBool = AtomicBool::new(false);

pub fn set_heap_ready() {
    HEAP_READY.store(true, Ordering::Release);
}

pub fn is_heap_ready() -> bool {
    HEAP_READY.load(Ordering::Acquire)
}

/// Возвращает человекочитаемое имя типа раздела MBR по его однобайтовому коду.
pub fn mbr_part_type_name(part_type: u8, lang_ru: bool) -> &'static str {
    match part_type {
        0x07 => "NTFS / exFAT",
        0x0B | 0x0C => "FAT32 LBA",
        0x0E => "FAT16 LBA",
        0x82 => {
            if lang_ru {
                "Раздел подкачки (Swap)"
            } else {
                "Linux Swap"
            }
        }
        0x83 => "Linux / Ext4",
        0xA5 => "FreeBSD / UFS2",
        0xEF => {
            if lang_ru {
                "Системный раздел EFI"
            } else {
                "EFI System Partition"
            }
        }
        _ => {
            if lang_ru {
                "Неизвестный тип"
            } else {
                "Unknown"
            }
        }
    }
}

/// Записывает сообщение этапа загрузки в консоль, COM1 и буфер /dev/kmsg в классическом Unix/BSD стиле.
pub fn log_event(subsystem: &str, args: fmt::Arguments) {
    if subsystem.is_empty() {
        crate::drivers::serial::_print(args);
        crate::drivers::display::_print(args);
        crate::kprint!("\n");

        if is_heap_ready() {
            let mut line = alloc::string::String::new();
            let _ = write!(line, "{}", args);
            crate::fs::devfs::mem::push_kmsg(&line);
        }
        return;
    }

    crate::kprint!("{}: ", subsystem);
    crate::drivers::serial::_print(args);
    crate::drivers::display::_print(args);
    crate::kprint!("\n");

    if is_heap_ready() {
        let mut line = alloc::string::String::new();
        let _ = write!(line, "{}: {}", subsystem, args);
        crate::fs::devfs::mem::push_kmsg(&line);
    }
}

#[macro_export]
macro_rules! kboot {
    ($sub:expr, $($arg:tt)*) => {
        $crate::bootlog::log_event($sub, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_mbr_part_type_name() {
        assert_eq!(mbr_part_type_name(0x0C, true), "FAT32 LBA");
        assert_eq!(mbr_part_type_name(0x83, true), "Linux / Ext4");
        assert_eq!(mbr_part_type_name(0xA5, true), "FreeBSD / UFS2");
        assert_eq!(mbr_part_type_name(0x82, true), "Раздел подкачки (Swap)");
        assert_eq!(mbr_part_type_name(0x82, false), "Linux Swap");
    }
}
