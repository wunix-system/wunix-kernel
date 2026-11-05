/// Структуры контекстов спецификации xHCI (Device Context, Input Context, Slot & Endpoint Context).

#[repr(C, align(32))]
#[derive(Clone, Copy, Debug, Default)]
pub struct SlotContext {
    pub dw0: u32,
    pub dw1: u32,
    pub dw2: u32,
    pub dw3: u32,
    pub reserved: [u32; 4],
}

impl SlotContext {
    pub fn init(&mut self, root_port: u8, speed: u8, context_entries: u8) {
        // dw0: Route String (0), Speed (bits 23:20), Context Entries (bits 31:27)
        self.dw0 = ((speed as u32 & 0x0F) << 20) | ((context_entries as u32 & 0x1F) << 27);
        // dw1: Root Hub Port Number (bits 23:16)
        self.dw1 = (root_port as u32 & 0xFF) << 16;
        self.dw2 = 0;
        self.dw3 = 0;
    }
}

#[repr(C, align(32))]
#[derive(Clone, Copy, Debug, Default)]
pub struct EndpointContext {
    pub dw0: u32,
    pub dw1: u32,
    pub tr_dequeue_lo: u32,
    pub tr_dequeue_hi: u32,
    pub dw4: u32,
    pub reserved: [u32; 3],
}

impl EndpointContext {
    /// Инициализирует контекст конечной точки.
    /// ep_type: 4 = Control Bi, 2 = Bulk Out, 6 = Bulk In, 3 = Interrupt Out, 7 = Interrupt In
    pub fn init(&mut self, ep_type: u8, max_packet_size: u16, ring_phys: u64, interval: u8) {
        // dw0: Interval (bits 23:16)
        self.dw0 = (interval as u32 & 0xFF) << 16;
        // dw1: CErr = 3 (bits 2:1), EP Type (bits 5:3), Max Packet Size (bits 31:16)
        self.dw1 = (3 << 1) | ((ep_type as u32 & 0x07) << 3) | ((max_packet_size as u32) << 16);
        // Dequeue pointer: bit 0 = DCS (Dequeue Cycle State = 1)
        self.tr_dequeue_lo = (ring_phys as u32) | 1;
        self.tr_dequeue_hi = (ring_phys >> 32) as u32;
        // Average TRB length = 8 (control/bulk)
        self.dw4 = 8;
    }
}

#[repr(C, align(32))]
#[derive(Clone, Copy, Debug, Default)]
pub struct InputControlContext {
    pub drop_flags: u32,
    pub add_flags: u32,
    pub reserved: [u32; 6],
}

#[repr(C, align(64))]
#[derive(Clone, Copy, Debug)]
pub struct InputContext {
    pub control: InputControlContext,
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

impl Default for InputContext {
    fn default() -> Self {
        Self {
            control: InputControlContext::default(),
            slot: SlotContext::default(),
            endpoints: [EndpointContext::default(); 31],
        }
    }
}

#[repr(C, align(64))]
#[derive(Clone, Copy, Debug)]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

impl Default for DeviceContext {
    fn default() -> Self {
        Self {
            slot: SlotContext::default(),
            endpoints: [EndpointContext::default(); 31],
        }
    }
}
