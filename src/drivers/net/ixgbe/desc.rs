/// Усовершенствованный дескриптор передачи (Advanced TX Descriptor) Intel 10GbE
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IxgbeAdvTxDesc {
    pub buffer_addr: u64,
    pub cmd_type_len: u32,
    pub olinfo_status: u32,
}

pub const IXGBE_TXD_CMD_EOP: u32 = 1 << 24;  // End of Packet
pub const IXGBE_TXD_CMD_IFCS: u32 = 1 << 25; // Insert FCS / CRC
pub const IXGBE_TXD_CMD_RS: u32 = 1 << 27;   // Report Status
pub const IXGBE_TXD_CMD_DEXT: u32 = 1 << 29; // Advanced Descriptor Extension
pub const IXGBE_TXD_STAT_DD: u32 = 1 << 0;   // Descriptor Done

/// Усовершенствованный дескриптор приема (Advanced RX Descriptor) Intel 10GbE
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IxgbeAdvRxDesc {
    pub pkt_addr: u64,
    pub hdr_addr: u64,
}

pub const IXGBE_RXD_STAT_DD: u32 = 1 << 0;  // Descriptor Done
pub const IXGBE_RXD_STAT_EOP: u32 = 1 << 1; // End of Packet

impl IxgbeAdvRxDesc {
    #[inline]
    pub fn is_done(&self) -> bool {
        let status = unsafe { (core::ptr::read_volatile(&self.pkt_addr) >> 32) as u32 };
        (status & IXGBE_RXD_STAT_DD) != 0
    }

    #[inline]
    pub fn packet_length(&self) -> usize {
        let hdr = unsafe { core::ptr::read_volatile(&self.hdr_addr) };
        (hdr & 0xFFFF) as usize
    }

    #[inline]
    pub fn reset_buffer(&mut self, buf_phys: u64) {
        unsafe {
            core::ptr::write_volatile(&mut self.pkt_addr, buf_phys);
            core::ptr::write_volatile(&mut self.hdr_addr, 0);
        }
    }
}
