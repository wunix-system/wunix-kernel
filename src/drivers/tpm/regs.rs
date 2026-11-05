// Смещения регистров TPM 2.0 CRB (Command Response Buffer) и TIS FIFO
pub const TPM_TIS_BASE_PHYS: u64 = 0xFED4_0000;
pub const TPM_CRB_BASE_PHYS: u64 = 0xFED4_0040;

// Регистры TIS
pub const TPM_ACCESS: usize = 0x00;
pub const TPM_STS: usize = 0x18;
pub const TPM_DATA_FIFO: usize = 0x24;

// Регистры CRB
pub const TPM_CRB_LOC_CTRL: usize = 0x08;
pub const TPM_CRB_LOC_STS: usize = 0x0C;
pub const TPM_CRB_CTRL_REQ: usize = 0x10;
pub const TPM_CRB_CTRL_STS: usize = 0x14;
pub const TPM_CRB_CTRL_CANCEL: usize = 0x18;
pub const TPM_CRB_CTRL_START: usize = 0x1C;
pub const TPM_CRB_CTRL_CMD_SIZE: usize = 0x28;
pub const TPM_CRB_CTRL_CMD_ADDR: usize = 0x2C;
pub const TPM_CRB_CTRL_RSP_SIZE: usize = 0x34;
pub const TPM_CRB_CTRL_RSP_ADDR: usize = 0x38;
