// Регистры SD Host Controller (SDHCI v1.0..v4.20)
pub const SDHCI_SDMA_ADDR: usize = 0x00;
pub const SDHCI_BLOCK_SIZE: usize = 0x04;
pub const SDHCI_BLOCK_COUNT: usize = 0x06;
pub const SDHCI_ARGUMENT: usize = 0x08;
pub const SDHCI_TRANSFER_MODE: usize = 0x0C;
pub const SDHCI_COMMAND: usize = 0x0E;
pub const SDHCI_RESPONSE_0: usize = 0x10;
pub const SDHCI_RESPONSE_1: usize = 0x14;
pub const SDHCI_RESPONSE_2: usize = 0x18;
pub const SDHCI_RESPONSE_3: usize = 0x1C;
pub const SDHCI_BUFFER_DATA: usize = 0x20;
pub const SDHCI_PRESENT_STATE: usize = 0x24;
pub const SDHCI_HOST_CONTROL: usize = 0x28;
pub const SDHCI_POWER_CONTROL: usize = 0x29;
pub const SDHCI_CLOCK_CONTROL: usize = 0x2C;
pub const SDHCI_TIMEOUT_CONTROL: usize = 0x2E;
pub const SDHCI_SOFTWARE_RESET: usize = 0x2F;
pub const SDHCI_INT_STATUS: usize = 0x30;
pub const SDHCI_INT_ENABLE: usize = 0x34;
pub const SDHCI_SIGNAL_ENABLE: usize = 0x38;

pub const SDHCI_RESET_ALL: u8 = 0x01;
pub const SDHCI_RESET_CMD: u8 = 0x02;
pub const SDHCI_RESET_DATA: u8 = 0x04;

pub const SDHCI_CMD_INHIBIT: u32 = 1 << 0;
pub const SDHCI_DATA_INHIBIT: u32 = 1 << 1;
pub const SDHCI_CARD_PRESENT: u32 = 1 << 16;

pub const SDHCI_INT_CMD_COMPLETE: u32 = 1 << 0;
pub const SDHCI_INT_TRANSFER_COMPLETE: u32 = 1 << 1;
pub const SDHCI_INT_BUFFER_READ_READY: u32 = 1 << 5;
pub const SDHCI_INT_BUFFER_WRITE_READY: u32 = 1 << 4;
