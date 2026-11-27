// Смещения регистров Broadcom Tigon3 BCM57xx
pub const TG3_MAC_MODE: usize = 0x0400;
pub const TG3_MAC_STATUS: usize = 0x0404;
pub const TG3_MAC_ADDR_0_HIGH: usize = 0x0410;
pub const TG3_MAC_ADDR_0_LOW: usize = 0x0414;
pub const TG3_TX_MAC_MODE: usize = 0x045C;
pub const TG3_RX_MAC_MODE: usize = 0x0468;
pub const TG3_BUF_MGR_MODE: usize = 0x4400;
pub const TG3_RDMA_MODE: usize = 0x4800;
pub const TG3_WDMA_MODE: usize = 0x4C00;
pub const TG3_FTQ_RESET: usize = 0x5C00;
pub const TG3_GRC_MISC_CFG: usize = 0x6804;

// Очереди дескрипторов хоста (Receive Buffer Descriptor Initiator)
pub const TG3_RCVDBDI_STD_BD: usize = 0x2450;
pub const TG3_RCVDBDI_MINI_BD: usize = 0x2460;

// Host Coalescing Control & Status Block
pub const TG3_HOSTCC_MODE: usize = 0x3C00;
pub const TG3_HOSTCC_STATUS_BLK_HOST_ADDR_HI: usize = 0x3C38;
pub const TG3_HOSTCC_STATUS_BLK_HOST_ADDR_LO: usize = 0x3C3C;
pub const TG3_HOSTCC_STATUS_BLK_NIC_ADDR: usize = 0x3C44;

// Очередь передачи хоста
pub const TG3_TX_PROD_RING_HOST_ADDR_HI: usize = 0x3800;
pub const TG3_TX_PROD_RING_HOST_ADDR_LO: usize = 0x3804;
pub const TG3_TX_PROD_NIC_RING: usize = 0x3808;

// Mailbox регистры хоста
pub const TG3_MAILBOX_RCV_STD_PROD_IDX: usize = 0x026C; // 0x268 + 4
pub const TG3_MAILBOX_RCVRET_CON_IDX_0: usize = 0x0284; // 0x280 + 4
pub const TG3_MAILBOX_SNDHOST_PROD_IDX_0: usize = 0x0304; // 0x300 + 4

// Адреса внутренней памяти SRAM
pub const TG3_NIC_SRAM_RCV_RET_RCB: usize = 0x8200;
pub const TG3_NIC_SRAM_SEND_RCB: usize = 0x8100;
pub const TG3_NIC_SRAM_STATUS_BLK: usize = 0x0B00;
pub const TG3_NIC_SRAM_RX_BUFFER_DESC: usize = 0x6000;
pub const TG3_NIC_SRAM_TX_BUFFER_DESC: usize = 0x4000;

// Флаги управления MAC
pub const TG3_MAC_MODE_ENABLE: u32 = 1 << 0;
pub const TG3_TX_MAC_ENABLE: u32 = 1 << 1;
pub const TG3_RX_MAC_ENABLE: u32 = 1 << 1;
pub const TG3_GRC_RESET_CORE: u32 = 1 << 0;

// Флаги Host Coalescing
pub const TG3_HOSTCC_MODE_ENABLE: u32 = 1 << 1;
pub const TG3_HOSTCC_MODE_NOW: u32 = 1 << 3;

// Флаги BDINFO
pub const TG3_BDINFO_FLAGS_DISABLED: u32 = 0x0002;
