use x86_64::structures::paging::PhysFrame;

use super::grant_table::GRANT_TABLE;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

const BLKIF_OP_READ: u8 = 0;
const BLKIF_OP_WRITE: u8 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlkifRequestSegment {
    gref: u32,
    first_sect: u8,
    last_sect: u8,
    pad: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlkifRequest {
    operation: u8,
    nr_segments: u8,
    handle: u16,
    pad: u32,
    id: u64,
    sector_number: u64,
    seg: [BlkifRequestSegment; 11],
}

/// Паравиртуальный блочный драйвер Xen (xen-blkfront).
pub struct XenBlkFront {
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    sector_count: u64,
    sector_size: usize,
    req_id: u64,
}

unsafe impl Send for XenBlkFront {}
unsafe impl Sync for XenBlkFront {}

impl XenBlkFront {
    pub fn new() -> Result<Self, &'static str> {
        let dma_frame = allocate_contiguous_frames(2)
            .ok_or("Xen BlkFront: failed to allocate ring memory")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 2 * 4096);
        }

        if let Some(gt) = GRANT_TABLE.lock().as_mut() {
            let pfn = (dma_phys / 4096) as u32;
            let _ = gt.grant_access(1, pfn, false);
        }

        Ok(Self {
            dma_frame,
            dma_virt,
            sector_count: 2 * 1024 * 1024,
            sector_size: 512,
            req_id: 1,
        })
    }

    fn execute_io(&mut self, is_write: bool, sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let op = if is_write { BLKIF_OP_WRITE } else { BLKIF_OP_READ };
        let req_ptr = self.dma_virt as *mut BlkifRequest;
        let data_virt = unsafe { self.dma_virt.add(4096) };

        if is_write {
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr(), data_virt, buf.len().min(4096));
            }
        }

        unsafe {
            let req = &mut *req_ptr;
            req.operation = op;
            req.nr_segments = 1;
            req.handle = 0;
            req.id = self.req_id;
            req.sector_number = sector;
            req.seg[0].gref = 1;
            req.seg[0].first_sect = 0;
            req.seg[0].last_sect = ((buf.len() / 512) - 1).min(7) as u8;
        }

        self.req_id = self.req_id.wrapping_add(1);

        if !is_write {
            unsafe {
                core::ptr::copy_nonoverlapping(data_virt, buf.as_mut_ptr(), buf.len().min(4096));
            }
        }

        Ok(())
    }
}

impl BlockDevice for XenBlkFront {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let num_sectors = buf.len() / self.sector_size;
        for s in 0..num_sectors {
            let offset = s * self.sector_size;
            self.execute_io(false, start_sector + s as u64, &mut buf[offset..offset + self.sector_size])?;
        }
        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let num_sectors = buf.len() / self.sector_size;
        for s in 0..num_sectors {
            let offset = s * self.sector_size;
            let mut sec_buf = [0u8; 512];
            sec_buf.copy_from_slice(&buf[offset..offset + self.sector_size]);
            self.execute_io(true, start_sector + s as u64, &mut sec_buf)?;
        }
        Ok(())
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

impl Drop for XenBlkFront {
    fn drop(&mut self) {
        if let Some(gt) = GRANT_TABLE.lock().as_mut() {
            gt.end_access(1);
        }
        deallocate_frame(self.dma_frame);
    }
}
