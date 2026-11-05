/// Регистры контроллера Realtek RTL8125 (2.5 Gigabit Ethernet)
pub const REG_MAC0: usize = 0x00;        // Аппаратный MAC-адрес (байты 0..5)
pub const REG_COMMAND: usize = 0x37;     // Регистр команд чипа (CR)
pub const REG_TPPOLL: usize = 0x38;      // Опрос очереди передачи (Transmit Priority Poll)
pub const REG_IMR: usize = 0x3C;         // Маска прерываний
pub const REG_ISR: usize = 0x3E;         // Статус прерываний
pub const REG_TCR: usize = 0x40;         // Управление передачей (Tx Config)
pub const REG_RCR: usize = 0x44;         // Управление приёмом (Rx Config)
pub const REG_RMS: usize = 0xDA;         // Rx Max Packet Size (16 бит)
pub const REG_MTPS: usize = 0xEC;        // Tx Max Packet Size
pub const REG_RDSAR_LO: usize = 0xE4;    // Базовый физический адрес кольца приёма (Low)
pub const REG_RDSAR_HI: usize = 0xE8;    // Базовый физический адрес кольца приёма (High)
pub const REG_TNPDS_LO: usize = 0x20;    // Базовый адрес кольца передачи (Low)
pub const REG_TNPDS_HI: usize = 0x24;    // Базовый адрес кольца передачи (High)

pub const CMD_RST: u8 = 1 << 4;          // Сброс чипа
pub const CMD_RE: u8 = 1 << 3;           // Включение приёмника
pub const CMD_TE: u8 = 1 << 2;           // Включение передатчика

pub const RCR_AAP: u32 = 1 << 0;         // All Physical (Promiscuous)
pub const RCR_APM: u32 = 1 << 1;         // Physical Match
pub const RCR_AM: u32 = 1 << 2;          // Multicast
pub const RCR_AB: u32 = 1 << 3;          // Broadcast
pub const RCR_MXDMA_UNLIMITED: u32 = 7 << 8;

pub const TCR_MXDMA_UNLIMITED: u32 = 7 << 8;
