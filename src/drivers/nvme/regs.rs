/// Смещения регистров контроллера NVMe в BAR0 MMIO (NVM Express 1.0..2.0)
pub const REG_CAP: usize = 0x00;      // Controller Capabilities (64 бит)
pub const REG_VS: usize = 0x08;       // Version (32 бит)
pub const REG_INTMS: usize = 0x0C;    // Interrupt Mask Set (32 бит)
pub const REG_INTMC: usize = 0x10;    // Interrupt Mask Clear (32 бит)
pub const REG_CC: usize = 0x14;       // Controller Configuration (32 бит)
pub const REG_CSTS: usize = 0x1C;     // Controller Status (32 бит)
pub const REG_NSSR: usize = 0x20;     // NVM Subsystem Reset (32 бит)
pub const REG_AQA: usize = 0x24;      // Admin Queue Attributes (32 бит)
pub const REG_ASQ: usize = 0x28;      // Admin Submission Queue Base Address (64 бит)
pub const REG_ACQ: usize = 0x30;      // Admin Completion Queue Base Address (64 бит)

pub const CC_EN: u32 = 1 << 0;        // Enable controller
pub const CC_CSS_NVM: u32 = 0 << 4;   // NVM Command Set
pub const CC_MPS_4K: u32 = 0 << 7;    // Memory Page Size: 4 KiB (2 ^ (12 + 0))
pub const CC_AMS_RR: u32 = 0 << 11;   // Round Robin arbitration
pub const CC_SHN_NONE: u32 = 0 << 14; // Shutdown notification: none
pub const CC_IOSQES_64: u32 = 6 << 16;// I/O Submission Queue Entry size: 2^6 = 64 байт
pub const CC_IOCQES_16: u32 = 4 << 20;// I/O Completion Queue Entry size: 2^4 = 16 байт

pub const CSTS_RDY: u32 = 1 << 0;     // Controller Ready (готов принимать команды)
pub const CSTS_CFS: u32 = 1 << 1;     // Controller Fatal Status (аппаратный сбой)

/// Вычисляет смещение регистра дверного звонка (Doorbell) для указанной очереди.
/// DSTRD (Doorbell Stride) задаётся в CAP битами 35:32 (значение `2 ^ (2 + dstrd)` байт).
/// qid: 0 для Admin Queue, 1+ для I/O Queues.
/// is_completion: false для Submission Queue Tail, true для Completion Queue Head.
#[inline]
pub fn doorbell_offset(dstrd: usize, qid: u32, is_completion: bool) -> usize {
    let stride = 4 << dstrd;
    let index = (2 * qid as usize) + if is_completion { 1 } else { 0 };
    0x1000 + (index * stride)
}
