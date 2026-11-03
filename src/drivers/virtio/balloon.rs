use alloc::boxed::Box;
use alloc::vec::Vec;
use x86_64::structures::paging::PhysFrame;

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

/// Драйвер динамического управления оперативной памятью VirtIO Balloon.
pub struct VirtioBalloon {
    transport: Box<dyn VirtioTransport>,
    inflate_queue: Virtqueue,
    deflate_queue: Virtqueue,
    ballooned_frames: Vec<PhysFrame>,
    dma_frame: PhysFrame,
    dma_virt: *mut u32,
}

unsafe impl Send for VirtioBalloon {}
unsafe impl Sync for VirtioBalloon {}

impl VirtioBalloon {
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();
        transport.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        let features = transport.read_device_features();
        transport.write_driver_features(features);
        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        transport.select_queue(0);
        let inf_sz = transport.get_queue_size();
        let inflate_queue = Virtqueue::new(0, inf_sz.min(32))?;
        transport.setup_queue(&inflate_queue);

        transport.select_queue(1);
        let def_sz = transport.get_queue_size();
        let deflate_queue = Virtqueue::new(1, def_sz.min(32))?;
        transport.setup_queue(&deflate_queue);

        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        let dma_frame = allocate_frame().ok_or(VirtioError::AllocationFailed)?;
        let dma_virt = (hhdm_offset() + dma_frame.start_address().as_u64()) as *mut u32;

        Ok(Self {
            transport,
            inflate_queue,
            deflate_queue,
            ballooned_frames: Vec::new(),
            dma_frame,
            dma_virt,
        })
    }

    pub fn target_pages(&mut self) -> u32 {
        let mut target = [0u8; 4];
        self.transport.read_device_config(0, &mut target);
        u32::from_le_bytes(target)
    }

    pub fn actual_pages(&mut self) -> u32 {
        let mut actual = [0u8; 4];
        self.transport.read_device_config(4, &mut actual);
        u32::from_le_bytes(actual)
    }

    pub fn inflate(&mut self, pages: usize) -> Result<(), VirtioError> {
        let count = pages.min(256);
        let mut pfns = Vec::with_capacity(count);

        for _ in 0..count {
            if let Some(frame) = allocate_frame() {
                let pfn = (frame.start_address().as_u64() / 4096) as u32;
                pfns.push(pfn);
                self.ballooned_frames.push(frame);
            } else {
                break;
            }
        }

        if pfns.is_empty() {
            return Ok(());
        }

        unsafe {
            core::ptr::copy_nonoverlapping(pfns.as_ptr(), self.dma_virt, pfns.len());
        }

        let vbuf = VirtioBuffer {
            phys_addr: self.dma_frame.start_address().as_u64(),
            len: (pfns.len() * 4) as u32,
            is_write: false,
        };

        let head = self.inflate_queue.add_buffer(&[vbuf])?;
        self.transport.notify_queue(0);

        while !self.inflate_queue.can_pop() {
            core::hint::spin_loop();
        }

        let _ = self.inflate_queue.pop_used();
        self.inflate_queue.free_chain(head);

        let actual = self.ballooned_frames.len() as u32;
        self.transport.write_device_config(4, &actual.to_le_bytes());

        Ok(())
    }

    pub fn deflate(&mut self, pages: usize) -> Result<(), VirtioError> {
        let count = pages.min(self.ballooned_frames.len()).min(256);
        let mut pfns = Vec::with_capacity(count);

        for _ in 0..count {
            if let Some(frame) = self.ballooned_frames.pop() {
                let pfn = (frame.start_address().as_u64() / 4096) as u32;
                pfns.push(pfn);
                deallocate_frame(frame);
            }
        }

        if pfns.is_empty() {
            return Ok(());
        }

        unsafe {
            core::ptr::copy_nonoverlapping(pfns.as_ptr(), self.dma_virt, pfns.len());
        }

        let vbuf = VirtioBuffer {
            phys_addr: self.dma_frame.start_address().as_u64(),
            len: (pfns.len() * 4) as u32,
            is_write: false,
        };

        let head = self.deflate_queue.add_buffer(&[vbuf])?;
        self.transport.notify_queue(1);

        while !self.deflate_queue.can_pop() {
            core::hint::spin_loop();
        }

        let _ = self.deflate_queue.pop_used();
        self.deflate_queue.free_chain(head);

        let actual = self.ballooned_frames.len() as u32;
        self.transport.write_device_config(4, &actual.to_le_bytes());

        Ok(())
    }
}

impl VirtioDevice for VirtioBalloon {
    fn name(&self) -> &str {
        "virtio-balloon"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Balloon
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl Drop for VirtioBalloon {
    fn drop(&mut self) {
        let total = self.ballooned_frames.len();
        let _ = self.deflate(total);
        self.reset();
        deallocate_frame(self.dma_frame);
    }
}
