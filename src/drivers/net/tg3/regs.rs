// Смещения регистров Broadcom Tigon3 BCM57xx
pub const TG3_MAC_MODE: usize = 0x0400;
pub const TG3_MAC_STATUS: usize = 0x0404;
pub const TG3_MAC_ADDR_0_HIGH: usize = 0x0410;
pub const TG3_MAC_ADDR_0_LOW: usize = 0x0414;
pub const TG3_FTQ_RESET: usize = 0x5C00;
pub const TG3_RX_PROD_RING_HOST_ADDR_HI: usize = 0x2450;
pub const TG3_RX_PROD_RING_HOST_ADDR_LO: usize = 0x2454;
pub const TG3_TX_PROD_RING_HOST_ADDR_HI: usize = 0x3800;
pub const TG3_TX_PROD_RING_HOST_ADDR_LO: usize = 0x3804;
pub const TG3_TX_PROD_NIC_RING: usize = 0x3808;

pub const TG3_MAC_MODE_ENABLE: u32 = 1 << 0;
pub const TG3_MAC_MODE_TXEN: u32 = 1 << 1;
pub const TG3_MAC_MODE_RXEN: u32 = 1 << 2;
