/// Смещения регистров UHCI (Port I/O от I/O Base в BAR4)
pub const REG_USBCMD: u16 = 0x00;
pub const REG_USBSTS: u16 = 0x02;
pub const REG_USBINTR: u16 = 0x04;
pub const REG_FRNUM: u16 = 0x06;
pub const REG_FLBASEADD: u16 = 0x08;
pub const REG_SOF: u16 = 0x0C;
pub const REG_PORTSC1: u16 = 0x10;
pub const REG_PORTSC2: u16 = 0x12;

pub const USBCMD_RS: u16 = 1 << 0;
pub const USBCMD_HCRESET: u16 = 1 << 1;
pub const USBCMD_CF: u16 = 1 << 6;
pub const USBCMD_MAXP: u16 = 1 << 7;

pub const PORTSC_CCS: u16 = 1 << 0;
pub const PORTSC_CSC: u16 = 1 << 1;
pub const PORTSC_PED: u16 = 1 << 2;
pub const PORTSC_PEDC: u16 = 1 << 3;
pub const PORTSC_LS: u16 = 1 << 8;
pub const PORTSC_PR: u16 = 1 << 9;

pub const PID_SETUP: u8 = 0x2D;
pub const PID_IN: u8 = 0x69;
pub const PID_OUT: u8 = 0xE1;
