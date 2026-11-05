/// Смещения регистров UHCI (Port I/O от I/O Base в BAR4)
pub const REG_USBCMD: u16 = 0x00;     // USB Command
pub const REG_USBSTS: u16 = 0x02;     // USB Status
pub const REG_USBINTR: u16 = 0x04;    // USB Interrupt Enable
pub const REG_FRNUM: u16 = 0x06;      // Frame Number
pub const REG_FLBASEADD: u16 = 0x08;  // Frame List Base Address (32 бит)
pub const REG_SOF: u16 = 0x0C;        // Start of Frame Modify
pub const REG_PORTSC1: u16 = 0x10;    // Port 1 Status/Control
pub const REG_PORTSC2: u16 = 0x12;    // Port 2 Status/Control

pub const USBCMD_RS: u16 = 1 << 0;    // Run/Stop
pub const USBCMD_HCRESET: u16 = 1 << 1;// Host Controller Reset
pub const USBCMD_MAXP: u16 = 1 << 7;  // Max Packet 64 bytes

pub const PORTSC_CCS: u16 = 1 << 0;   // Current Connect Status
pub const PORTSC_PED: u16 = 1 << 2;   // Port Enable/Disable
