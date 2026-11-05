/// Смещения регистров Intel High Definition Audio (HDA)
pub const REG_GCAP: usize = 0x00;        // Global Capabilities (16 бит)
pub const REG_VMIN: usize = 0x02;        // Minor Version
pub const REG_VMAJ: usize = 0x03;        // Major Version
pub const REG_GCTL: usize = 0x08;        // Global Control (бит 0: CRST)
pub const REG_WAKEEN: usize = 0x0C;      // Wake Enable
pub const REG_STATESTS: usize = 0x0E;    // State Change Status (флаги подключенных кодеков)

pub const REG_CORBLBASE: usize = 0x40;   // База CORB младшие 32 бита
pub const REG_CORBUBASE: usize = 0x44;   // База CORB старшие 32 бита
pub const REG_CORBWP: usize = 0x48;      // Указатель записи CORB
pub const REG_CORBRP: usize = 0x4A;      // Указатель чтения CORB
pub const REG_CORBCTL: usize = 0x4C;     // Управление CORB (бит 1: RUN)
pub const REG_CORBSIZE: usize = 0x4E;    // Размер CORB

pub const REG_RIRBLBASE: usize = 0x50;   // База RIRB младшие 32 бита
pub const REG_RIRBUBASE: usize = 0x54;   // База RIRB старшие 32 бита
pub const REG_RIRBWP: usize = 0x58;      // Указатель записи RIRB
pub const REG_RIRBCTL: usize = 0x5C;     // Управление RIRB (бит 1: DMAEN)
pub const REG_RIRBSIZE: usize = 0x5E;    // Размер RIRB

pub const GCTL_CRST: u32 = 1 << 0;       // Controller Reset (1 = рабочий режим, 0 = сброс)

/// Вычисляет смещение дескриптора потока данных (Stream Descriptor)
#[inline]
pub fn stream_offset(stream_idx: usize) -> usize {
    0x80 + (stream_idx * 0x20)
}

pub const SD_CTL: usize = 0x00;          // Stream Control (24 бит)
pub const SD_STS: usize = 0x03;          // Stream Status (8 бит)
pub const SD_CBL: usize = 0x08;          // Cyclic Buffer Length
pub const SD_LVI: usize = 0x0C;          // Last Valid Index
pub const SD_FMT: usize = 0x12;          // Stream Format
pub const SD_BDLPL: usize = 0x18;        // Buffer Descriptor List Pointer Low
pub const SD_BDLPU: usize = 0x1C;        // Buffer Descriptor List Pointer High

pub const SD_CTL_RUN: u32 = 1 << 1;      // Запуск DMA стрима
pub const SD_CTL_IOCE: u32 = 1 << 2;     // Прерывание по завершении буфера
