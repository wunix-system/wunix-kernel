use crate::memory::hhdm_offset;
use super::regs::*;

pub struct TpmCrb {
    base: u64,
}

impl TpmCrb {
    pub fn new(mmio_base: u64) -> Self {
        Self {
            base: mmio_base,
        }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    #[inline]
    fn write32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, val) }
    }

    /// Запрашивает аппаратную локальность 0 (Locality 0 Request)
    pub fn request_locality(&self) -> bool {
        self.write32(TPM_CRB_LOC_CTRL, 1);
        for _ in 0..10_000 {
            if (self.read32(TPM_CRB_LOC_STS) & 1) != 0 {
                return true;
            }
        }
        false
    }

    /// Отправляет команду TPM 2.0 в командный буфер и ожидает ответа
    pub fn execute_command(&self, cmd: &[u8], rsp: &mut [u8]) -> Result<usize, &'static str> {
        let cmd_addr = self.read32(TPM_CRB_CTRL_CMD_ADDR) as u64;
        let rsp_addr = self.read32(TPM_CRB_CTRL_RSP_ADDR) as u64;

        if cmd_addr == 0 || rsp_addr == 0 {
            return Err("tpm_crb: unconfigured command/response buffers");
        }

        let cmd_virt = hhdm_offset() + cmd_addr;
        let rsp_virt = hhdm_offset() + rsp_addr;

        unsafe {
            core::ptr::copy_nonoverlapping(cmd.as_ptr(), cmd_virt as *mut u8, cmd.len());
        }
        self.write32(TPM_CRB_CTRL_CMD_SIZE, cmd.len() as u32);

        // Запуск выполнения команды (START)
        self.write32(TPM_CRB_CTRL_START, 1);

        for _ in 0..100_000 {
            if self.read32(TPM_CRB_CTRL_START) == 0 {
                let rsp_size = self.read32(TPM_CRB_CTRL_RSP_SIZE) as usize;
                let copy_len = rsp_size.min(rsp.len());
                unsafe {
                    core::ptr::copy_nonoverlapping(rsp_virt as *const u8, rsp.as_mut_ptr(), copy_len);
                }
                return Ok(copy_len);
            }
        }

        Err("tpm_crb: command timeout")
    }
}
