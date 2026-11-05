/// Смещения Capability регистров xHCI
pub const CAP_CAPLENGTH: usize = 0x00;   // Байт длины регистров возможностей
pub const CAP_HCVERSION: usize = 0x02;   // Версия спецификации xHCI (BCD)
pub const CAP_HCSPARAMS1: usize = 0x04;  // Параметры хоста 1 (MaxSlots, MaxPorts)
pub const CAP_HCSPARAMS2: usize = 0x08;  // Параметры хоста 2 (Scratchpad buffers)
pub const CAP_HCCPARAMS1: usize = 0x10;  // Возможности ядра (64-бит адресация, CSZ)
pub const CAP_DBOFF: usize = 0x14;       // Смещение массива дверных звонков
pub const CAP_RTSOFF: usize = 0x18;      // Смещение регистров рантайма

/// Смещения Operational регистров (относительно `base + CAPLENGTH`)
pub const OP_USBCMD: usize = 0x00;       // Команды USB (Run/Stop, Reset)
pub const OP_USBSTS: usize = 0x04;       // Статус USB (Halted, Host System Error, CNR)
pub const OP_PAGESIZE: usize = 0x08;     // Размер поддерживаемых страниц памяти
pub const OP_DNCTRL: usize = 0x14;       // Контроль уведомлений устройств
pub const OP_CRCR: usize = 0x18;         // Command Ring Control Register (64 бит)
pub const OP_DCBAAP: usize = 0x30;       // Device Context Base Address Array (64 бит)
pub const OP_CONFIG: usize = 0x38;       // Конфигурация слотов (MaxSlotsEn)

pub const USBCMD_RS: u32 = 1 << 0;       // Run/Stop
pub const USBCMD_HCRST: u32 = 1 << 1;    // Host Controller Reset
pub const USBCMD_INTE: u32 = 1 << 2;     // Interrupter Enable

pub const USBSTS_HCH: u32 = 1 << 0;      // HC Halted (контроллер остановлен)
pub const USBSTS_CNR: u32 = 1 << 11;     // Controller Not Ready

pub const PORTSC_CCS: u32 = 1 << 0;      // Current Connect Status (устройство подключено)
pub const PORTSC_PED: u32 = 1 << 1;      // Port Enabled/Disabled
pub const PORTSC_PR: u32 = 1 << 4;       // Port Reset
pub const PORTSC_PP: u32 = 1 << 9;       // Port Power

/// Вычисляет смещение регистра PORTSC для порта с 1-based индексом
#[inline]
pub fn portsc_offset(port_index_1based: usize) -> usize {
    0x400 + (port_index_1based - 1) * 0x10
}
