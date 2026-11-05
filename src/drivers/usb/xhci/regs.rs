/// Смещения Capability регистров xHCI
pub const CAP_CAPLENGTH: usize = 0x00;
pub const CAP_HCVERSION: usize = 0x02;
pub const CAP_HCSPARAMS1: usize = 0x04;
pub const CAP_HCSPARAMS2: usize = 0x08;
pub const CAP_HCCPARAMS1: usize = 0x10;
pub const CAP_DBOFF: usize = 0x14;
pub const CAP_RTSOFF: usize = 0x18;

/// Смещения Operational регистров (относительно `base + CAPLENGTH`)
pub const OP_USBCMD: usize = 0x00;
pub const OP_USBSTS: usize = 0x04;
pub const OP_PAGESIZE: usize = 0x08;
pub const OP_DNCTRL: usize = 0x14;
pub const OP_CRCR: usize = 0x18;
pub const OP_DCBAAP: usize = 0x30;
pub const OP_CONFIG: usize = 0x38;

pub const USBCMD_RS: u32 = 1 << 0;       // Run/Stop
pub const USBCMD_HCRST: u32 = 1 << 1;    // Host Controller Reset
pub const USBCMD_INTE: u32 = 1 << 2;     // Interrupter Enable

pub const USBSTS_HCH: u32 = 1 << 0;      // HC Halted (контроллер остановлен)
pub const USBSTS_CNR: u32 = 1 << 11;     // Controller Not Ready

pub const PORTSC_CCS: u32 = 1 << 0;      // Current Connect Status
pub const PORTSC_PED: u32 = 1 << 1;      // Port Enabled/Disabled
pub const PORTSC_PR: u32 = 1 << 4;       // Port Reset
pub const PORTSC_PLS_MASK: u32 = 0x0F << 5;
pub const PORTSC_PP: u32 = 1 << 9;       // Port Power
pub const PORTSC_SPEED_MASK: u32 = 0x0F << 10;
pub const PORTSC_PRC: u32 = 1 << 21;     // Port Reset Change
pub const PORTSC_CSC: u32 = 1 << 17;     // Connect Status Change

/// Смещения регистров прерываний рантайма (Interrupter 0)
pub const RT_IMAN: usize = 0x20;
pub const RT_IMOD: usize = 0x24;
pub const RT_ERSTSZ: usize = 0x28;
pub const RT_ERSTBA: usize = 0x30;
pub const RT_ERDP: usize = 0x38;

/// Коды завершения TRB событий (TRB Completion Codes)
pub const CC_SUCCESS: u32 = 1;
pub const CC_DATA_BUFFER_ERROR: u32 = 2;
pub const CC_BABBLE: u32 = 3;
pub const CC_USB_TRANSACTION_ERROR: u32 = 4;
pub const CC_TRB_ERROR: u32 = 5;
pub const CC_STALL: u32 = 6;
pub const CC_RESOURCE_ERROR: u32 = 7;
pub const CC_SHORT_PACKET: u32 = 13;

/// Вычисляет смещение регистра PORTSC для порта с 1-based индексом
#[inline]
pub fn portsc_offset(port_index_1based: usize) -> usize {
    0x400 + (port_index_1based - 1) * 0x10
}
