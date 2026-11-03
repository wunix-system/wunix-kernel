use x86_64::structures::paging::PhysFrame;

use super::grant_table::GRANT_TABLE;
use crate::drivers::traits::NetDevice;
use crate::drivers::virtio::VirtioError;
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NetifTxRequest {
    gref: u32,
    offset: u16,
    flags: u16,
    id: u16,
    size: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NetifRxResponse {
    id: u16,
    offset: u16,
    flags: u16,
    status: i16,
}

/// Паравиртуальный сетевой драйвер Xen (xen-netfront).
pub struct XenNetFront {
    mac: [u8; 6],
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    tx_id: u16,
}

unsafe impl Send for XenNetFront {}
unsafe impl Sync for XenNetFront {}

impl XenNetFront {
    pub fn new() -> Result<Self, &'static str> {
        let dma_frame = allocate_contiguous_frames(2)
            .ok_or("Xen NetFront: failed to allocate ring memory")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 2 * 4096);
        }

        if let Some(gt) = GRANT_TABLE.lock().as_mut() {
            let pfn = (dma_phys / 4096) as u32;
            let _ = gt.grant_access(2, pfn, false);
        }

        Ok(Self {
            mac: [0x00, 0x16, 0x3E, 0x11, 0x22, 0x33],
            dma_frame,
            dma_virt,
            tx_id: 1,
        })
    }
}

impl NetDevice for XenNetFront {
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), VirtioError> {
        if packet.len() > 1514 {
            return Err(VirtioError::IoError);
        }

        let tx_req_ptr = self.dma_virt as *mut NetifTxRequest;
        let tx_data_ptr = unsafe { self.dma_virt.add(2048) };

        unsafe {
            core::ptr::copy_nonoverlapping(packet.as_ptr(), tx_data_ptr, packet.len());
            let req = &mut *tx_req_ptr;
            req.gref = 2;
            req.offset = 2048;
            req.flags = 0;
            req.id = self.tx_id;
            req.size = packet.len() as u16;
        }

        self.tx_id = self.tx_id.wrapping_add(1);
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, VirtioError> {
        let rx_resp_ptr = unsafe { self.dma_virt.add(1024) } as *const NetifRxResponse;
        let rx_resp = unsafe { *rx_resp_ptr };

        if rx_resp.status > 0 {
            let len = (rx_resp.status as usize).min(buf.len());
            let rx_data_ptr = unsafe { self.dma_virt.add(2048 + rx_resp.offset as usize) };
            unsafe {
                core::ptr::copy_nonoverlapping(rx_data_ptr, buf.as_mut_ptr(), len);
            }
            Ok(Some(len))
        } else {
            Ok(None)
        }
    }
}

impl Drop for XenNetFront {
    fn drop(&mut self) {
        if let Some(gt) = GRANT_TABLE.lock().as_mut() {
            gt.end_access(2);
        }
        deallocate_frame(self.dma_frame);
    }
}
