use crate::drivers::virt::vbox::vmmdev::{
    VmmDevRequestHeader, VBOX_GUEST, VMMDEV_REQ_REPORT_GUEST_INFO,
};

const VBOX_OSTYPE_LINUX26_X64: u32 = 0x53000;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VBoxGuestInfo {
    pub header: VmmDevRequestHeader,
    pub interface_version: u32,
    pub os_type: u32,
}

pub fn report_guest_os() -> Result<(), &'static str> {
    let mut guest_guard = VBOX_GUEST.lock();
    let guest = guest_guard.as_mut().ok_or("VBox VMMDev not initialized")?;

    unsafe {
        let req = guest.dma_ptr::<VBoxGuestInfo>();
        *req = VBoxGuestInfo {
            header: VmmDevRequestHeader::new(
                core::mem::size_of::<VBoxGuestInfo>() as u32,
                VMMDEV_REQ_REPORT_GUEST_INFO,
            ),
            interface_version: 0x0001_0004,
            os_type: VBOX_OSTYPE_LINUX26_X64,
        };

        let rc = guest.execute_request()?;
        if rc >= 0 {
            Ok(())
        } else {
            Err("VBox report guest OS failed")
        }
    }
}

pub fn sync_time_with_host() -> Option<u64> {
    let mut guest_guard = VBOX_GUEST.lock();
    let guest = guest_guard.as_mut()?;
    guest.get_host_time().ok()
}
