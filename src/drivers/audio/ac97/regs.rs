/// Регистры микшера AC'97 (NAMBAR, BAR0)
pub const MIXER_RESET: u16 = 0x00;
pub const MIXER_MASTER_VOL: u16 = 0x02;
pub const MIXER_PCM_VOL: u16 = 0x18;

/// Регистры Bus Master аудиоконтроллера AC'97 (NABMBAR, BAR1)
pub const BM_PO_BDBAR: u16 = 0x10; // Buffer Descriptor Base Address (32 бит)
pub const BM_PO_CIV: u16 = 0x14;   // Current Index Value (8 бит)
pub const BM_PO_LVI: u16 = 0x15;   // Last Valid Index (8 бит)
pub const BM_PO_SR: u16 = 0x16;    // Status Register (16 бит)
pub const BM_PO_CR: u16 = 0x1B;    // Control Register (8 бит)
pub const BM_GLB_CTRL: u16 = 0x2C; // Global Control (32 бит)
pub const BM_GLB_STAT: u16 = 0x30; // Global Status (32 бит)

pub const PO_CR_RUN: u8 = 1 << 0;  // Запуск DMA воспроизведения
pub const PO_CR_RESET: u8 = 1 << 1;// Сброс регистров воспроизведения
pub const PO_CR_IOCE: u8 = 1 << 3; // Прерывание по окончанию буфера

pub const PO_SR_DCH: u16 = 1 << 0; // DMA Controller Halted
pub const PO_SR_CELV: u16 = 1 << 1;// Текущий буфер равен последнему (завершение)
