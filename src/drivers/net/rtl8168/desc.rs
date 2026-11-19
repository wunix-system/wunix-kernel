/// 16-байтный дескриптор передачи/приема Realtek RTL8168/8111 PCIe
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RtlDesc {
    pub opts1: u32,
    pub opts2: u32,
    pub buf_addr: u64,
}

const _: () = assert!(core::mem::size_of::<RtlDesc>() == 16);

pub const DESC_OWN: u32 = 1 << 31; // 1 = принадлежит контроллеру, 0 = процессору
pub const DESC_EOR: u32 = 1 << 30; // End of Ring (последний дескриптор в кольце)
pub const DESC_FS: u32 = 1 << 29;  // First Segment
pub const DESC_LS: u32 = 1 << 28;  // Last Segment
