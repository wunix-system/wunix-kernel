/// Смещения Capability регистров EHCI
pub const CAP_CAPLENGTH: usize = 0x00;
pub const CAP_HCSPARAMS: usize = 0x04;
pub const CAP_HCCPARAMS: usize = 0x08;

/// Смещения Operational регистров EHCI (база = `base + CAPLENGTH`)
pub const OP_USBCMD: usize = 0x00;
pub const OP_USBSTS: usize = 0x04;
pub const OP_USBINTR: usize = 0x08;
pub const OP_FRINDEX: usize = 0x0C;
pub const OP_CTRLDSSEGMENT: usize = 0x10;
pub const OP_PERIODICLISTBASE: usize = 0x14;
pub const OP_ASYNCLISTADDR: usize = 0x18;
pub const OP_CONFIGFLAG: usize = 0x40;
pub const OP_PORTSC_BASE: usize = 0x44;

pub const USBCMD_RS: u32 = 1 << 0;     // Run/Stop
pub const USBCMD_HCRESET: u32 = 1 << 1;// Host Controller Reset
pub const USBCMD_ASE: u32 = 1 << 5;    // Asynchronous Schedule Enable

pub const USBSTS_HCHALTED: u32 = 1 << 12;

pub const PORTSC_CONNECT: u32 = 1 << 0;
pub const PORTSC_ENABLE: u32 = 1 << 2;
pub const PORTSC_POWER: u32 = 1 << 12;
