pub const HBA_GHC_AE: u32 = 1 << 31;

pub const PORT_CMD_ST: u32 = 1 << 0;
pub const PORT_CMD_FRE: u32 = 1 << 4;
pub const PORT_CMD_FR: u32 = 1 << 14;
pub const PORT_CMD_CR: u32 = 1 << 15;

pub const ATA_DEV_BUSY: u32 = 1 << 7;
pub const ATA_DEV_DRQ: u32 = 1 << 3;
pub const ATA_DEV_ERR: u32 = 1 << 0;

pub const SATA_SIG_ATA: u32 = 0x0000_0101;
pub const SATA_SIG_ATAPI: u32 = 0xEB14_0101;

/// 32-байтный заголовок команды AHCI (Command Header)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandHeader {
    pub flags: u16,        // cfl (биты 4:0), w (бит 6), prdtl (биты 31:16)
    pub prdtl: u16,        // Physical Region Descriptor Table Length
    pub prdbc: u32,        // Physical Region Descriptor Byte Count
    pub ctba: u32,         // Command Table Base Address Low
    pub ctbau: u32,        // Command Table Base Address High
    pub reserved: [u32; 4],
}

const _: () = assert!(core::mem::size_of::<CommandHeader>() == 32);

/// 16-байтная запись таблицы регионов данных (PRDT Entry)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PrdtEntry {
    pub dba: u32,          // Data Base Address Low
    pub dbau: u32,         // Data Base Address High
    pub reserved: u32,
    pub dbc_ioc: u32,      // Byte Count (биты 21:0) и Interrupt on Completion (бит 31)
}

const _: () = assert!(core::mem::size_of::<PrdtEntry>() == 16);
