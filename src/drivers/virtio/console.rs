use alloc::boxed::Box;
use x86_64::structures::paging::PhysFrame;

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

/// Драйвер виртуальной консоли VirtIO Console (/dev/hvc0).
pub struct VirtioConsole {
    transport: Box<dyn VirtioTransport>,
    rx_queue: Virtqueue,
    tx_queue: Virtqueue,
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
}

unsafe impl Send for VirtioConsole {}
unsafe impl Sync for VirtioConsole {}

impl VirtioConsole {
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();
        transport.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        let features = transport.read_device_features();
        transport.write_driver_features(features);
        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        transport.select_queue(0);
        let rx_sz = transport.get_queue_size();
        let rx_queue = Virtqueue::new(0, rx_sz.min(32))?;
        transport.setup_queue(&rx_queue);

        transport.select_queue(1);
        let tx_sz = transport.get_queue_size();
        let tx_queue = Virtqueue::new(1, tx_sz.min(32))?;
        transport.setup_queue(&tx_queue);

        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        let dma_frame = allocate_contiguous_frames(2).ok_or(VirtioError::AllocationFailed)?;
        let dma_virt = (hhdm_offset() + dma_frame.start_address().as_u64()) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 2 * 4096);
        }

        let mut cons = Self {
            transport,
            rx_queue,
            tx_queue,
            dma_frame,
            dma_virt,
        };

        cons.fill_rx_buffer()?;
        Ok(cons)
    }

    fn fill_rx_buffer(&mut self) -> Result<(), VirtioError> {
        let rx_phys = self.dma_frame.start_address().as_u64();
        let buf = VirtioBuffer {
            phys_addr: rx_phys,
            len: 4096,
            is_write: true,
        };
        self.rx_queue.add_buffer(&[buf])?;
        self.transport.notify_queue(0);
        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, VirtioError> {
        if data.is_empty() {
            return Ok(0);
        }

        let len = data.len().min(4096);
        let tx_phys = self.dma_frame.start_address().as_u64() + 4096;
        let tx_virt = unsafe { self.dma_virt.add(4096) };

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), tx_virt, len);
        }

        let vbuf = VirtioBuffer {
            phys_addr: tx_phys,
            len: len as u32,
            is_write: false,
        };

        let head = self.tx_queue.add_buffer(&[vbuf])?;
        self.transport.notify_queue(1);

        let mut attempts = 0;
        while !self.tx_queue.can_pop() {
            core::hint::spin_loop();
            attempts += 1;
            if attempts > 100_000 {
                break;
            }
        }

        let _ = self.tx_queue.pop_used();
        self.tx_queue.free_chain(head);
        Ok(len)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, VirtioError> {
        if let Some((head, bytes_written)) = self.rx_queue.pop_used() {
            let actual = (bytes_written as usize).min(buf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(self.dma_virt, buf.as_mut_ptr(), actual);
            }
            self.rx_queue.free_chain(head);
            let _ = self.fill_rx_buffer();
            Ok(Some(actual))
        } else {
            Ok(None)
        }
    }
}

impl VirtioDevice for VirtioConsole {
    fn name(&self) -> &str {
        "virtio-console"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Console
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl Drop for VirtioConsole {
    fn drop(&mut self) {
        self.reset();
        deallocate_frame(self.dma_frame);
    }
}
