/// Дескриптор команды передачи Intel Wi-Fi FH DMA
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct IwlTxDesc {
    pub dma_addr: u64,
    pub length: u32,
    pub flags: u32,
}

/// Дескриптор приема пакета Intel Wi-Fi FH DMA
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct IwlRxDesc {
    pub dma_addr: u64,
    pub status: u32,
    pub length: u32,
}
