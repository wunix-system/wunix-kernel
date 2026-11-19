/// 16-байтный блок запроса передачи (Transfer Request Block) спецификации xHCI
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Trb {
    pub data_ptr: u64,
    pub status: u32,
    pub control: u32,
}

const _: () = assert!(core::mem::size_of::<Trb>() == 16);

pub const TRB_TYPE_NORMAL: u32 = 1;
pub const TRB_TYPE_SETUP: u32 = 2;
pub const TRB_TYPE_DATA: u32 = 3;
pub const TRB_TYPE_STATUS: u32 = 4;
pub const TRB_TYPE_LINK: u32 = 6;
pub const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub const TRB_TYPE_DISABLE_SLOT: u32 = 10;
pub const TRB_TYPE_ADDRESS_DEV: u32 = 11;
pub const TRB_TYPE_CONFIG_EP: u32 = 12;

pub const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
pub const TRB_TYPE_CMD_COMPLETION: u32 = 33;
pub const TRB_TYPE_PORT_CHANGE: u32 = 34;

pub const TRB_CYCLE: u32 = 1 << 0;
pub const TRB_TC: u32 = 1 << 1;     // Toggle Cycle (для Link TRB)
pub const TRB_IOC: u32 = 1 << 5;    // Interrupt On Completion

impl Trb {
    #[inline]
    pub fn new(data_ptr: u64, status: u32, control: u32) -> Self {
        Self { data_ptr, status, control }
    }

    #[inline]
    pub fn trb_type(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    #[inline]
    pub fn cycle_bit(&self) -> bool {
        (self.control & TRB_CYCLE) != 0
    }
}
