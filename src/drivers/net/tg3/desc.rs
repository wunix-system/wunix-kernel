/// Дескриптор передачи Broadcom Tigon3 (Send Buffer Descriptor - 16 байт)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tg3TxDesc {
    pub addr_hi: u32,
    pub addr_lo: u32,
    pub len_flags: u32,
    pub vlan_tag: u32,
}

pub const TG3_TX_FLAG_END: u32 = 1 << 2; // Конец пакета

/// Дескриптор стандартного кольца приема Broadcom Tigon3 (Receive Buffer Descriptor - 32 байта)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tg3RxDesc {
    pub addr_hi: u32,
    pub addr_lo: u32,
    pub idx_len: u32,
    pub type_flags: u32,
    pub ip_tcp_csum: u32,
    pub err_vlan: u32,
    pub reserved: u32,
    pub opaque: u32,
}

pub const RXD_FLAG_END: u32 = 0x0004;
pub const RXD_OPAQUE_RING_STD: u32 = 0x00010000;
pub const RXD_OPAQUE_INDEX_MASK: u32 = 0x0000FFFF;

/// Блок статуса Broadcom Tigon3 (Hardware Status Block - 80 байт)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tg3HwStatus {
    pub status: u32,
    pub status_tag: u32,
    pub rx_jumbo_consumer: u16,
    pub rx_consumer: u16,
    pub rx_mini_consumer: u16,
    pub reserved: u16,
    pub idx: [Tg3StatusIdx; 16],
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tg3StatusIdx {
    pub rx_producer: u16,
    pub tx_consumer: u16,
}
