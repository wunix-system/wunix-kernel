/// Дескриптор передачи Broadcom Tigon3 (Send Buffer Descriptor)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Tg3TxDesc {
    pub addr_hi: u32,
    pub addr_lo: u32,
    pub len_flags: u32,
    pub vlan_tag: u32,
}

pub const TG3_TX_FLAG_END: u32 = 1 << 2; // Конец пакета

/// Дескриптор приема Broadcom Tigon3 (Receive Buffer Descriptor)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Tg3RxDesc {
    pub addr_hi: u32,
    pub addr_lo: u32,
    pub idx_len: u32,
    pub type_flags: u32,
}
