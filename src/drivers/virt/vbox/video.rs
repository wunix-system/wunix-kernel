use x86_64::instructions::port::Port;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::DisplayDevice;
use crate::drivers::virtio::VirtioError;
use crate::memory::map_kernel_mmio;

const VBE_DISPI_IOPORT_INDEX: u16 = 0x01CE;
const VBE_DISPI_IOPORT_DATA: u16 = 0x01CF;

const VBE_DISPI_INDEX_XRES: u16 = 0x1;
const VBE_DISPI_INDEX_YRES: u16 = 0x2;
const VBE_DISPI_INDEX_BPP: u16 = 0x3;
const VBE_DISPI_INDEX_ENABLE: u16 = 0x4;
const VBE_DISPI_INDEX_BANK: u16 = 0x5;

const VBE_DISPI_DISABLED: u16 = 0x00;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;

/// Драйвер виртуального видеоконтроллера VirtualBox / VBoxSVGA / Bochs VBE.
pub struct VBoxVideoDevice {
    width: u32,
    height: u32,
    pitch: u32,
    framebuffer_virt: *mut u32,
}

unsafe impl Send for VBoxVideoDevice {}
unsafe impl Sync for VBoxVideoDevice {}

impl VBoxVideoDevice {
    fn write_reg(index: u16, value: u16) {
        unsafe {
            let mut idx_port = Port::<u16>::new(VBE_DISPI_IOPORT_INDEX);
            let mut data_port = Port::<u16>::new(VBE_DISPI_IOPORT_DATA);
            idx_port.write(index);
            data_port.write(value);
        }
    }

    pub fn new(pci: &PciDevice, width: u32, height: u32) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_io_space();

        let lfb_bar = pci.first_memory_bar().ok_or("VBox Video: BAR0 LFB not found")?;
        let lfb_size = (width as usize) * (height as usize) * 4;

        let virt_addr = map_kernel_mmio(lfb_bar.address, lfb_size)
            .map_err(|_| "VBox Video: failed to map LFB MMIO")?;

        Self::write_reg(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);
        Self::write_reg(VBE_DISPI_INDEX_XRES, width as u16);
        Self::write_reg(VBE_DISPI_INDEX_YRES, height as u16);
        Self::write_reg(VBE_DISPI_INDEX_BPP, 32);
        Self::write_reg(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED);
        Self::write_reg(VBE_DISPI_INDEX_BANK, 0);

        let fb_ptr = virt_addr.as_mut_ptr::<u32>();

        Ok(Self {
            width,
            height,
            pitch: width,
            framebuffer_virt: fb_ptr,
        })
    }
}

impl DisplayDevice for VBoxVideoDevice {
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

    fn flush(&mut self) -> Result<(), VirtioError> {
        Ok(())
    }
}
