/// Структура формата LBA в дескрипторе неймспейса NVMe
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LbaFormat {
    pub ms: u16,         // Metadata Size
    pub ds: u8,          // LBA Data Size (степень двойки: 9 = 512 байт, 12 = 4096 байт)
    pub rp: u8,          // Relative Performance
}

/// 4096-байтная структура Identify Namespace
#[repr(C)]
pub struct IdentifyNamespace {
    pub nsze: u64,       // Namespace Size (общее число секторов LBA)
    pub ncap: u64,       // Namespace Capacity
    pub nuse: u64,       // Namespace Utilization
    pub nsfeat: u8,
    pub nlbaf: u8,       // Number of LBA Formats
    pub flbas: u8,       // Formatted LBA Size (индекс активного формата в lbaf[0..15])
    pub mc: u8,
    pub dpc: u8,
    pub dps: u8,
    pub nmic: u8,
    pub rescap: u8,
    pub fpi: u8,
    pub dlfeat: u8,
    pub nawun: u16,
    pub nawupf: u16,
    pub nacwu: u16,
    pub nabsn: u16,
    pub nabo: u16,
    pub nabspf: u16,
    pub noiob: u16,
    pub nvmcap: [u8; 16],
    pub reserved0: [u8; 40],
    pub nguid: [u8; 16],
    pub eui64: [u8; 8],
    pub lbaf: [LbaFormat; 16], // Список поддерживаемых геометрий блоков
    pub reserved1: [u8; 3904],
}

const _: () = assert!(core::mem::size_of::<IdentifyNamespace>() == 4096);

impl IdentifyNamespace {
    /// Возвращает размер логического сектора диска в байтах (обычно 512 или 4096)
    pub fn sector_size(&self) -> usize {
        let fmt_idx = (self.flbas & 0x0F) as usize;
        let ds = self.lbaf[fmt_idx].ds;
        if ds >= 9 && ds <= 16 {
            1usize << ds
        } else {
            512 // Безопасное значение по умолчанию для legacy LBA
        }
    }
}
