/// 64-байтный дескриптор команды отправки NVMe (Submission Queue Entry)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NvmeCmd {
    pub opcode: u8,
    pub flags: u8,
    pub cid: u16,           // Command Identifier
    pub nsid: u32,          // Namespace Identifier
    pub reserved0: u64,
    pub mptr: u64,          // Metadata Pointer
    pub prp1: u64,          // Physical Region Page 1
    pub prp2: u64,          // Physical Region Page 2
    pub cdw10: u32,         // Command Dword 10
    pub cdw11: u32,         // Command Dword 11
    pub cdw12: u32,         // Command Dword 12
    pub cdw13: u32,         // Command Dword 13
    pub cdw14: u32,         // Command Dword 14
    pub cdw15: u32,         // Command Dword 15
}

const _: () = assert!(core::mem::size_of::<NvmeCmd>() == 64);

/// 16-байтный дескриптор завершения команды NVMe (Completion Queue Entry)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NvmeCqe {
    pub result: u32,        // Command-specific result
    pub reserved: u32,
    pub sq_head: u16,       // Submission Queue Head Pointer
    pub sq_id: u16,         // Submission Queue Identifier
    pub cid: u16,           // Command Identifier
    pub status: u16,        // Status Field (бит 0: Phase Tag 'P')
}

const _: () = assert!(core::mem::size_of::<NvmeCqe>() == 16);

impl NvmeCqe {
    /// Проверяет фазовый бит (Phase Tag). Контроллер инвертирует его при каждом проходе кольца.
    #[inline]
    pub fn is_phase_match(&self, expected_phase: bool) -> bool {
        let phase = (self.status & 1) != 0;
        phase == expected_phase
    }

    /// Проверяет статус завершения (Status Code Type и Status Code). 0 означает успех.
    #[inline]
    pub fn is_success(&self) -> bool {
        ((self.status >> 1) & 0x7FF) == 0
    }
}
