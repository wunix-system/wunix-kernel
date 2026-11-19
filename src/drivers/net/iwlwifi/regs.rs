// Регистры Intel Wi-Fi (AX200, AX201, AX210, 9260, 8265, 7265)
pub const CSR_HW_IF_CONFIG_REG: usize = 0x000;
pub const CSR_INT: usize = 0x008;
pub const CSR_INT_MASK: usize = 0x00C;
pub const CSR_RESET: usize = 0x020;
pub const CSR_GP_CNTRL: usize = 0x024;

pub const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 1 << 0;
pub const CSR_GP_CNTRL_REG_FLAG_INIT_DONE: u32 = 1 << 2;
pub const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ: u32 = 1 << 3;
pub const CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP: u32 = 1 << 4;

// Flow Handler (FH) DMA регистры
pub const FH_TX_CONFIG: usize = 0x1C00;
pub const FH_TX_BASE_ADDR: usize = 0x1C04;
pub const FH_RX_CONFIG: usize = 0x1C40;
pub const FH_RX_BASE_ADDR: usize = 0x1C44;
pub const FH_RX_WPTR: usize = 0x1C48;
pub const FH_RX_STATUS: usize = 0x1C4C;
