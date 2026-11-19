/// Смещения регистров Open Host Controller Interface (OHCI 1.0)
pub const REG_REVISION: usize = 0x00;
pub const REG_CONTROL: usize = 0x04;
pub const REG_COMMAND_STATUS: usize = 0x08;
pub const REG_INTERRUPT_STATUS: usize = 0x0C;
pub const REG_INTERRUPT_ENABLE: usize = 0x10;
pub const REG_INTERRUPT_DISABLE: usize = 0x14;
pub const REG_HCCA: usize = 0x18;
pub const REG_PERIOD_CURRENT_ED: usize = 0x1C;
pub const REG_CONTROL_HEAD_ED: usize = 0x20;
pub const REG_CONTROL_CURRENT_ED: usize = 0x24;
pub const REG_BULK_HEAD_ED: usize = 0x28;
pub const REG_BULK_CURRENT_ED: usize = 0x2C;
pub const REG_DONE_HEAD: usize = 0x30;
pub const REG_FM_INTERVAL: usize = 0x34;
pub const REG_FM_REMAINING: usize = 0x38;
pub const REG_FM_NUMBER: usize = 0x3C;
pub const REG_PERIODIC_START: usize = 0x40;
pub const REG_LS_THRESHOLD: usize = 0x44;
pub const REG_RH_DESCRIPTOR_A: usize = 0x48;
pub const REG_RH_DESCRIPTOR_B: usize = 0x4C;
pub const REG_RH_STATUS: usize = 0x50;
pub const REG_RH_PORT_STATUS_BASE: usize = 0x54;

/// Битовые флаги регистра Control
pub const CONTROL_CBSR_MASK: u32 = 0x03;
pub const CONTROL_PLE: u32 = 1 << 2;  // Periodic List Enable
pub const CONTROL_IE: u32 = 1 << 3;   // Isochronous Enable
pub const CONTROL_CLE: u32 = 1 << 4;  // Control List Enable
pub const CONTROL_BLE: u32 = 1 << 5;  // Bulk List Enable
pub const CONTROL_HCFS_OPERATIONAL: u32 = 2 << 6;
pub const CONTROL_HCFS_RESET: u32 = 0 << 6;

/// Битовые флаги регистра CommandStatus
pub const CMD_HCR: u32 = 1 << 0;      // Host Controller Reset

/// Битовые флаги портов корневого концентратора (Root Hub Port Status)
pub const RH_PORT_CCS: u32 = 1 << 0;  // Current Connect Status
pub const RH_PORT_PES: u32 = 1 << 1;  // Port Enable Status
pub const RH_PORT_PRS: u32 = 1 << 4;  // Port Reset Status
pub const RH_PORT_PPS: u32 = 1 << 9;  // Port Power Status
pub const RH_PORT_LSDA: u32 = 1 << 9; // Low Speed Device Attached
