use alloc::boxed::Box;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

/// Драйвер генератора случайных чисел VirtIO RNG (Entropy Device).
pub struct VirtioRng {
    transport: Box<dyn VirtioTransport>,
    queue: Virtqueue,
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
}

unsafe impl Send for VirtioRng {}
unsafe impl Sync for VirtioRng {}

impl VirtioRng {
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();
        transport.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        let features = transport.read_device_features();
        transport.write_driver_features(features);
        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        transport.select_queue(0);
        let q_size = transport.get_queue_size();
        if q_size == 0 {
            return Err(VirtioError::QueueTooSmall);
        }

        let queue = Virtqueue::new(0, q_size.min(64))?;
        transport.setup_queue(&queue);
        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        let dma_frame = allocate_frame().ok_or(VirtioError::AllocationFailed)?;
        let dma_virt = (hhdm_offset() + dma_frame.start_address().as_u64()) as *mut u8;

        Ok(Self {
            transport,
            queue,
            dma_frame,
            dma_virt,
        })
    }

    pub fn get_random_bytes(&mut self, buf: &mut [u8]) -> Result<usize, VirtioError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let fetch_len = buf.len().min(4096) as u32;
        let phys_addr = self.dma_frame.start_address().as_u64();

        let vbuf = VirtioBuffer {
            phys_addr,
            len: fetch_len,
            is_write: true,
        };

        let head = self.queue.add_buffer(&[vbuf])?;
        self.transport.notify_queue(0);

        let mut attempts = 0;
        while !self.queue.can_pop() {
            core::hint::spin_loop();
            attempts += 1;
            if attempts > 100_000 {
                return Err(VirtioError::IoError);
            }
        }

        if let Some((_, len_written)) = self.queue.pop_used() {
            let actual = (len_written as usize).min(buf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(self.dma_virt, buf.as_mut_ptr(), actual);
            }
            self.queue.free_chain(head);
            Ok(actual)
        } else {
            self.queue.free_chain(head);
            Err(VirtioError::IoError)
        }
    }
}

impl VirtioDevice for VirtioRng {
    fn name(&self) -> &str {
        "virtio-rng"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Entropy
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl Drop for VirtioRng {
    fn drop(&mut self) {
        self.reset();
        deallocate_frame(self.dma_frame);
    }
}

pub static VIRTIO_RNG: Mutex<Option<VirtioRng>> = Mutex::new(None);

pub fn fill_entropy(buf: &mut [u8]) -> bool {
    if let Some(rng) = VIRTIO_RNG.lock().as_mut() {
        rng.get_random_bytes(buf).is_ok()
    } else {
        false
    }
}
