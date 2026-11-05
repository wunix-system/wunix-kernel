pub mod crb;
pub mod regs;

use crb::TpmCrb;
use spin::Mutex;

static TPM_INSTANCE: Mutex<Option<TpmCrb>> = Mutex::new(None);

/// Инициализирует модуль безопасности TPM 2.0 (CRB / Intel PTT / AMD fTPM)
pub fn init() {
    let mmio = match crate::memory::map_kernel_mmio(regs::TPM_CRB_BASE_PHYS, 4096) {
        Ok(v) => v.as_u64(),
        Err(_) => {
            crate::klog!("tpm: TPM 2.0 module not present");
            return;
        }
    };

    let tpm = TpmCrb::new(mmio);
    if tpm.request_locality() {
        crate::klog!("tpm: TPM 2.0 security module detected and activated (Locality 0)");
        *TPM_INSTANCE.lock() = Some(tpm);
    } else {
        crate::klog!("tpm: TPM 2.0 module not present or inactive");
    }
}

/// Генерирует криптографически стойкие псевдослучайные байты через аппаратный генератор TPM 2.0
pub fn get_random(buf: &mut [u8]) -> Result<(), &'static str> {
    let tpm_guard = TPM_INSTANCE.lock();
    let tpm = tpm_guard.as_ref().ok_or("tpm: not available")?;

    // Команда TPM2_GetRandom: tag 0x8001, bytes_requested
    let count = buf.len().min(32) as u16;
    let cmd = [
        0x80, 0x01, // TPM_ST_NO_SESSIONS
        0x00, 0x00, 0x00, 0x0C, // Command Size: 12 bytes
        0x00, 0x00, 0x01, 0x7B, // Command Code: TPM_CC_GetRandom (0x17B)
        (count >> 8) as u8, (count & 0xFF) as u8,
    ];

    let mut rsp = [0u8; 64];
    let len = tpm.execute_command(&cmd, &mut rsp)?;
    if len > 14 {
        let copy_len = (len - 14).min(buf.len());
        buf[..copy_len].copy_from_slice(&rsp[14..14 + copy_len]);
        Ok(())
    } else {
        Err("tpm: invalid response")
    }
}
