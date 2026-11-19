/// Смещения регистров Realtek RTL8168/8111 (MMIO / Port I/O)
pub const REG_MAC0: u16 = 0x00;        // MAC адрес (байты 0..3)
pub const REG_MAC4: u16 = 0x04;        // MAC адрес (байты 4..5)
pub const REG_TX_DESC_LO: u16 = 0x20;  // Младшие 32 бита базы кольца TX
pub const REG_TX_DESC_HI: u16 = 0x24;  // Старшие 32 бита базы кольца TX
pub const REG_COMMAND: u16 = 0x37;     // Регистр команд чипа
pub const REG_TX_POLL: u16 = 0x38;     // Опрос очередей передачи
pub const REG_IMR: u16 = 0x3C;         // Маска прерываний
pub const REG_ISR: u16 = 0x3E;         // Статус прерываний
pub const REG_TCR: u16 = 0x40;         // Настройки передатчика
pub const REG_RCR: u16 = 0x44;         // Настройки приемника
pub const REG_RX_DESC_LO: u16 = 0xE4;  // Младшие 32 бита базы кольца RX (RDSAR)
pub const REG_RX_DESC_HI: u16 = 0xE8;  // Старшие 32 бита базы кольца RX
pub const REG_RMS: u16 = 0xDA;         // Максимальный размер пакета приема (RMS)
pub const REG_MTPS: u16 = 0xEC;        // Максимальный размер пакета передачи (MTPS)

pub const CMD_RST: u8 = 1 << 4;        // Сброс контроллера
pub const CMD_RE: u8 = 1 << 3;         // Включение приемника
pub const CMD_TE: u8 = 1 << 2;         // Включение передатчика

pub const RCR_AAP: u32 = 1 << 0;
pub const RCR_APM: u32 = 1 << 1;
pub const RCR_AM: u32 = 1 << 2;
pub const RCR_AB: u32 = 1 << 3;
pub const RCR_MXDMA_UNLIM: u32 = 7 << 8; // Безлимитный DMA burst
