/// 16-байтный дескриптор пакета Realtek RTL8125 2.5GbE
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rtl8125Desc {
    pub opts1: u32,
    pub opts2: u32,
    pub buf_addr: u64,
}

pub const DESC_OWN: u32 = 1 << 31; // 1 = дескриптор принадлежит NIC, 0 = процессору
pub const DESC_EOR: u32 = 1 << 30; // End of Ring (зацикливание кольца)
pub const DESC_FS: u32 = 1 << 29;  // First Segment
pub const DESC_LS: u32 = 1 << 28;  // Last Segment
