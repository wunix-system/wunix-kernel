use crate::drivers::virt::vbox::vmmdev::{
    VmmDevRequestHeader, VBOX_GUEST, VMMDEV_REQ_HGCM_CALL, VMMDEV_REQ_HGCM_CONNECT,
    VMMDEV_REQ_HGCM_DISCONNECT,
};

pub const SHFL_FN_QUERY_MAP_INFO: u32 = 1;
pub const SHFL_FN_DISCONNECT: u32 = 2;
pub const SHFL_FN_CREATE: u32 = 3;
pub const SHFL_FN_CLOSE: u32 = 4;
pub const SHFL_FN_READ: u32 = 5;
pub const SHFL_FN_WRITE: u32 = 6;
pub const SHFL_FN_LIST: u32 = 8;
pub const SHFL_FN_INFORMATION: u32 = 9;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HgcmParm {
    pub parm_type: u32,
    pub val_u64: u64,
    pub val_extra: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmConnect {
    pub header: VmmDevRequestHeader,
    pub loc_type: u32,
    pub service_name: [u8; 128],
    pub client_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmDisconnect {
    pub header: VmmDevRequestHeader,
    pub client_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmCall {
    pub header: VmmDevRequestHeader,
    pub client_id: u32,
    pub function: u32,
    pub c_parms: u32,
    pub parms: [HgcmParm; 6],
}

/// Клиент Host-Guest Communication Manager (HGCM) для обмена RPC с хостом.
pub struct HgcmClient {
    client_id: u32,
}

impl HgcmClient {
    pub fn connect(service_name: &str) -> Result<Self, &'static str> {
        let mut guest_guard = VBOX_GUEST.lock();
        let guest = guest_guard.as_mut().ok_or("VBox VMMDev not initialized")?;

        let mut name_buf = [0u8; 128];
        let bytes = service_name.as_bytes();
        let len = bytes.len().min(127);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        unsafe {
            let req = guest.dma_ptr::<VmmDevHgcmConnect>();
            *req = VmmDevHgcmConnect {
                header: VmmDevRequestHeader::new(
                    core::mem::size_of::<VmmDevHgcmConnect>() as u32,
                    VMMDEV_REQ_HGCM_CONNECT,
                ),
                loc_type: 1,
                service_name: name_buf,
                client_id: 0,
            };

            let rc = guest.execute_request()?;
            if rc >= 0 && (*req).client_id != 0 {
                Ok(Self {
                    client_id: (*req).client_id,
                })
            } else {
                Err("HGCM connect failed")
            }
        }
    }

    pub fn call(&self, function: u32, parms: &[HgcmParm]) -> Result<i32, &'static str> {
        let mut guest_guard = VBOX_GUEST.lock();
        let guest = guest_guard.as_mut().ok_or("VBox VMMDev not initialized")?;

        let mut parm_array = [HgcmParm {
            parm_type: 0,
            val_u64: 0,
            val_extra: 0,
        }; 6];
        let count = parms.len().min(6);
        parm_array[..count].copy_from_slice(&parms[..count]);

        unsafe {
            let req = guest.dma_ptr::<VmmDevHgcmCall>();
            *req = VmmDevHgcmCall {
                header: VmmDevRequestHeader::new(
                    core::mem::size_of::<VmmDevHgcmCall>() as u32,
                    VMMDEV_REQ_HGCM_CALL,
                ),
                client_id: self.client_id,
                function,
                c_parms: count as u32,
                parms: parm_array,
            };

            guest.execute_request()
        }
    }

    pub fn client_id(&self) -> u32 {
        self.client_id
    }
}

impl Drop for HgcmClient {
    fn drop(&mut self) {
        if let Some(guest) = VBOX_GUEST.lock().as_mut() {
            unsafe {
                let req = guest.dma_ptr::<VmmDevHgcmDisconnect>();
                *req = VmmDevHgcmDisconnect {
                    header: VmmDevRequestHeader::new(
                        core::mem::size_of::<VmmDevHgcmDisconnect>() as u32,
                        VMMDEV_REQ_HGCM_DISCONNECT,
                    ),
                    client_id: self.client_id,
                };
                let _ = guest.execute_request();
            }
        }
    }
}
