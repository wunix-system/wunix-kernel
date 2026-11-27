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
pub const SHFL_FN_MAP_FOLDER: u32 = 12;
pub const SHFL_FN_LIST: u32 = 8;
pub const SHFL_FN_INFORMATION: u32 = 9;
pub const SHFL_FN_REMOVE: u32 = 10;

pub const VMMDEV_HGCM_PARM_TYPE_32BIT: u32 = 1;
pub const VMMDEV_HGCM_PARM_TYPE_64BIT: u32 = 2;
pub const VMMDEV_HGCM_PARM_TYPE_LINADDR: u32 = 4;

pub const VMMDEV_HGCM_REQ_DONE: u32 = 1 << 0;
pub const VINF_HGCM_ASYNC_EXECUTE: i32 = 2903;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HgcmParm {
    pub parm_type: u32,
    pub val_u64: u64,
    pub val_extra: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmRequestHeader {
    pub header: VmmDevRequestHeader,
    pub flags: u32,
    pub result: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmConnect {
    pub hgcm_header: VmmDevHgcmRequestHeader,
    pub loc_type: u32,
    pub service_name: [u8; 128],
    pub client_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmDisconnect {
    pub hgcm_header: VmmDevHgcmRequestHeader,
    pub client_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHgcmCall {
    pub hgcm_header: VmmDevHgcmRequestHeader,
    pub client_id: u32,
    pub function: u32,
    pub c_parms: u32,
    pub parms: [HgcmParm; 6],
}

fn hgcm_hdr(size: u32, req_type: u32) -> VmmDevHgcmRequestHeader {
    VmmDevHgcmRequestHeader {
        header: VmmDevRequestHeader::new(size, req_type),
        flags: 0,
        result: 0,
    }
}

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
                hgcm_header: hgcm_hdr(
                    core::mem::size_of::<VmmDevHgcmConnect>() as u32,
                    VMMDEV_REQ_HGCM_CONNECT,
                ),
                loc_type: 2, // VMMDevHGCMLoc_LocalHost_Existing
                service_name: name_buf,
                client_id: 0,
            };

            let rc = guest.execute_request()?;
            if rc == VINF_HGCM_ASYNC_EXECUTE {
                for _ in 0..100_000 {
                    let fl = core::ptr::read_volatile(&(*req).hgcm_header.flags);
                    if (fl & VMMDEV_HGCM_REQ_DONE) != 0 {
                        break;
                    }
                    core::hint::spin_loop();
                }
            }
            let res = core::ptr::read_volatile(&(*req).hgcm_header.result);
            let cid = core::ptr::read_volatile(&(*req).client_id);
            if rc >= 0 && res >= 0 && cid != 0 {
                Ok(Self { client_id: cid })
            } else {
                Err("HGCM connect failed")
            }
        }
    }

    pub fn call(&self, function: u32, parms: &mut [HgcmParm]) -> Result<i32, &'static str> {
        let mut guest_guard = VBOX_GUEST.lock();
        let guest = guest_guard.as_mut().ok_or("VBox VMMDev not initialized")?;

        let mut parm_array = [HgcmParm::default(); 6];
        let count = parms.len().min(6);
        parm_array[..count].copy_from_slice(&parms[..count]);

        unsafe {
            let req = guest.dma_ptr::<VmmDevHgcmCall>();
            *req = VmmDevHgcmCall {
                hgcm_header: hgcm_hdr(
                    core::mem::size_of::<VmmDevHgcmCall>() as u32,
                    VMMDEV_REQ_HGCM_CALL,
                ),
                client_id: self.client_id,
                function,
                c_parms: count as u32,
                parms: parm_array,
            };

            let rc = guest.execute_request()?;
            if rc == VINF_HGCM_ASYNC_EXECUTE {
                for _ in 0..200_000 {
                    let fl = core::ptr::read_volatile(&(*req).hgcm_header.flags);
                    if (fl & VMMDEV_HGCM_REQ_DONE) != 0 {
                        break;
                    }
                    core::hint::spin_loop();
                }
            }
            for i in 0..count {
                parms[i] = core::ptr::read_volatile(&(*req).parms[i]);
            }
            let result = core::ptr::read_volatile(&(*req).hgcm_header.result);
            Ok(result)
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
                    hgcm_header: hgcm_hdr(
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
