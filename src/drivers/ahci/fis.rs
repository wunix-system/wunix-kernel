/// Типы Frame Information Structure (FIS) спецификации Serial ATA
pub const FIS_TYPE_REG_H2D: u8 = 0x27;       // Register FIS - Host to Device
pub const FIS_TYPE_REG_D2H: u8 = 0x34;       // Register FIS - Device to Host
pub const FIS_TYPE_DMA_ACT: u8 = 0x39;       // DMA Activate FIS
pub const FIS_TYPE_DMA_SETUP: u8 = 0x41;     // DMA Setup FIS
pub const FIS_TYPE_DATA: u8 = 0x46;          // Data FIS
pub const FIS_TYPE_BIST: u8 = 0x58;          // BIST Activate FIS
pub const FIS_TYPE_PIO_SETUP: u8 = 0x5F;     // PIO Setup FIS
pub const FIS_TYPE_DEV_BITS: u8 = 0xA1;      // Set Device Bits FIS

/// 20-байтный заголовок FIS Host-to-Device для передачи ATA команд
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FisRegH2D {
    pub fis_type: u8,      // 0x27
    pub pmport_c: u8,      // Бит 7: Command (1) / Control (0)
    pub command: u8,       // Код команды ATA (0xEC, 0x25, 0x35)
    pub feature_lo: u8,
    pub lba0: u8,
    pub lba1: u8,
    pub lba2: u8,
    pub device: u8,        // Бит 6: LBA mode (1)
    pub lba3: u8,
    pub lba4: u8,
    pub lba5: u8,
    pub feature_hi: u8,
    pub count_lo: u8,
    pub count_hi: u8,
    pub icc: u8,
    pub control: u8,
    pub reserved: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<FisRegH2D>() == 20);
