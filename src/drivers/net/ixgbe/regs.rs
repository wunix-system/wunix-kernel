// Регистры управления и статуса Intel 10 Gigabit (82599, X540, X550)
pub const IXGBE_CTRL: usize = 0x00000;
pub const IXGBE_STATUS: usize = 0x00008;
pub const IXGBE_CTRL_EXT: usize = 0x00018;

pub const IXGBE_CTRL_RST: u32 = 1 << 26; // Device Reset
pub const IXGBE_CTRL_SLU: u32 = 1 << 3;  // Set Link Up

pub const IXGBE_STATUS_LINK_UP: u32 = 1 << 7;
pub const IXGBE_STATUS_SPEED_10G: u32 = 3 << 13;

// Регистры прерываний
pub const IXGBE_EICR: usize = 0x00800;
pub const IXGBE_EICS: usize = 0x00808;
pub const IXGBE_EIMS: usize = 0x00880;
pub const IXGBE_EIMC: usize = 0x00888;

// Регистры приемника (RX Queue 0)
pub const IXGBE_RDBAL: usize = 0x01000;
pub const IXGBE_RDBAH: usize = 0x01004;
pub const IXGBE_RDLEN: usize = 0x01008;
pub const IXGBE_RDH: usize = 0x01010;
pub const IXGBE_RDT: usize = 0x01018;
pub const IXGBE_RXCTRL: usize = 0x03000;
pub const IXGBE_FCTRL: usize = 0x05008;

pub const IXGBE_RXCTRL_RXEN: u32 = 1 << 0;
pub const IXGBE_FCTRL_BAM: u32 = 1 << 10; // Broadcast Accept Mode
pub const IXGBE_FCTRL_MPE: u32 = 1 << 8;  // Multicast Promiscuous Enable
pub const IXGBE_FCTRL_UPE: u32 = 1 << 9;  // Unicast Promiscuous Enable

// Регистры передатчика (TX Queue 0)
pub const IXGBE_TDBAL: usize = 0x06000;
pub const IXGBE_TDBAH: usize = 0x06004;
pub const IXGBE_TDLEN: usize = 0x06008;
pub const IXGBE_TDH: usize = 0x06010;
pub const IXGBE_TDT: usize = 0x06018;
pub const IXGBE_DMATXCTL: usize = 0x04A80;
pub const IXGBE_DMATXCTL_TE: u32 = 1 << 0; // Transmit Enable

// Регистры фильтрации MAC-адреса (Receive Address 0)
pub const IXGBE_RAL: usize = 0x0A200;
pub const IXGBE_RAH: usize = 0x0A204;
pub const IXGBE_RAH_AV: u32 = 1 << 31; // Address Valid
