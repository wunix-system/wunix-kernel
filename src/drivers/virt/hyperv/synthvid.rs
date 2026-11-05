use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{DisplayDevice, DisplayDeviceError};
use crate::memory::map_kernel_mmio;

pub const MICROSOFT_PCI_VENDOR_ID: u16 = 0x1414;
pub const HYPERV_SYNTHVID_DEVICE_ID: u16 = 0x5353;

/// Драйвер синтетического видеоадаптера Microsoft Hyper-V Synthetic Video.
pub struct HyperVSynthVidDevice {
    width: u32,
    height: u32,
    pitch: u32,
    framebuffer_virt: *mut u32,
}

unsafe impl Send for HyperVSynthVidDevice {}
unsafe impl Sync for HyperVSynthVidDevice {}

impl HyperVSynthVidDevice {
    pub fn new(pci: &PciDevice, width: u32, height: u32) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("Hyper-V SynthVid: BAR0 Framebuffer not found")?;
        let lfb_size = (width as usize) * (height as usize) * 4;

        let fb_virt = map_kernel_mmio(bar0.address, lfb_size)
            .map_err(|_| "Hyper-V SynthVid: failed to map Framebuffer MMIO")?;
        let fb_ptr = fb_virt.as_mut_ptr::<u32>();

        Ok(Self {
            width,
            height,
            pitch: width,
            framebuffer_virt: fb_ptr,
        })
    }
}

impl DisplayDevice for HyperVSynthVidDevice {
    fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let offset = (y * self.pitch + x) as usize;
            unsafe {
                core::ptr::write_volatile(self.framebuffer_virt.add(offset), color);
            }
        }
    }

    fn flush(&mut self) -> Result<(), DisplayDeviceError> {
        Ok(())
    }
}
