/// Смещения портов ввода-вывода сетевого чипа Realtek RTL8139 (I/O Base)
pub const REG_IDR0: u16 = 0x00;       // MAC-адрес (байты 0..3)
pub const REG_IDR4: u16 = 0x04;       // MAC-адрес (байты 4..5)
pub const REG_MAR0: u16 = 0x08;       // Multicast filter mask
pub const REG_TSD0: u16 = 0x10;       // Transmit Status/Command дескриптор 0
pub const REG_TSD1: u16 = 0x14;       // Transmit Status/Command дескриптор 1
pub const REG_TSD2: u16 = 0x18;       // Transmit Status/Command дескриптор 2
pub const REG_TSD3: u16 = 0x1C;       // Transmit Status/Command дескриптор 3
pub const REG_TSAD0: u16 = 0x20;      // Физический адрес буфера передачи 0
pub const REG_TSAD1: u16 = 0x24;      // Физический адрес буфера передачи 1
pub const REG_TSAD2: u16 = 0x28;      // Физический адрес буфера передачи 2
pub const REG_TSAD3: u16 = 0x2C;      // Физический адрес буфера передачи 3
pub const REG_RBSTART: u16 = 0x30;    // Физический адрес кольцевого буфера приема RX
pub const REG_CR: u16 = 0x37;         // Command Register
pub const REG_CAPR: u16 = 0x38;       // Current Address of Packet Read
pub const REG_CBR: u16 = 0x3A;        // Current Buffer Address
pub const REG_IMR: u16 = 0x3C;        // Interrupt Mask Register
pub const REG_ISR: u16 = 0x3E;        // Interrupt Status Register
pub const REG_TCR: u16 = 0x40;        // Transmit Configuration Register
pub const REG_RCR: u16 = 0x44;        // Receive Configuration Register
pub const REG_CONFIG1: u16 = 0x52;    // Configuration Register 1

pub const CR_BUFE: u8 = 1 << 0;       // Буфер приема пуст
pub const CR_TE: u8 = 1 << 2;         // Передатчик включен (Transmitter Enable)
pub const CR_RE: u8 = 1 << 3;         // Приемник включен (Receiver Enable)
pub const CR_RST: u8 = 1 << 4;        // Программный сброс (Reset)

pub const RCR_AAP: u32 = 1 << 0;      // Принимать все пакеты (promiscuous)
pub const RCR_APM: u32 = 1 << 1;      // Принимать пакеты для нашего MAC
pub const RCR_AM: u32 = 1 << 2;       // Принимать мультикаст
pub const RCR_AB: u32 = 1 << 3;       // Принимать широковещательные пакеты (Broadcast)
pub const RCR_WRAP: u32 = 1 << 7;     // Заворачивать пакет в буфер при переполнении кольца

pub const TSD_OWN: u32 = 1 << 13;     // DMA передача завершена контроллером
pub const TSD_TOK: u32 = 1 << 15;     // Пакет успешно передан в кабель
