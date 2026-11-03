use x86_64::instructions::port::Port;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::DisplayDevice;
use crate::drivers::virtio::VirtioError;
use crate::memory::map_kernel_mmio;

pub const VMWARE_PCI_VENDOR_ID: u16 = 0x15AD;
pub const VMWARE_SVGA2_DEVICE_ID: u16 = 0x0405;

const SVGA_ID_2: u32 = 0x9000_0002;

const SVGA_REG_ID: u32 = 0;
const SVGA_REG_ENABLE: u32 = 1;
const SVGA_REG_WIDTH: u32 = 2;
const SVGA_REG_HEIGHT: u32 = 3;
const SVGA_REG_BITS_PER_PIXEL: u32 = 7;
const SVGA_REG_BYTES_PER_LINE: u32 = 9;
const SVGA_REG_SYNC: u32 = 21;
const SVGA_REG_BUSY: u32 = 22;

/// Драйвер видеоконтроллера VMware SVGA II.
pub struct VmwareSvgaDevice {
    io_base: u16,
    width: u32,
    height: u32,
    pitch: u32,
    framebuffer_virt: *mut u32,
}

unsafe impl Send for VmwareSvgaDevice {}
unsafe impl Sync for VmwareSvgaDevice {}

impl VmwareSvgaDevice {
    fn write_reg(&self, reg: u32, value: u32) {
        unsafe {
            let mut idx = Port::<u32>::new(self.io_base);
            let mut val = Port::<u32>::new(self.io_base + 1);
            idx.write(reg);
            val.write(value);
        }
    }

    fn read_reg(&self, reg: u32) -> u32 {
        unsafe {
            let mut idx = Port::<u32>::new(self.io_base);
            let mut val = Port::<u32>::new(self.io_base + 1);
            idx.write(reg);
            val.read()
        }
    }

    pub fn new(pci: &PciDevice, width: u32, height: u32) -> Result<Self, &'static str> {
        pci.enable_io_space();
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let io_bar = pci.first_io_bar().ok_or("VMware SVGA: IO BAR not found")?;
        let fb_bar = pci.bar(1).ok_or("VMware SVGA: FB BAR1 not found")?;

        let io_base = io_bar.address as u16;
        let mut dev = Self {
            io_base,
            width,
            height,
            pitch: width,
            framebuffer_virt: core::ptr::null_mut(),
        };

        dev.write_reg(SVGA_REG_ID, SVGA_ID_2);
        let id = dev.read_reg(SVGA_REG_ID);
        if id != SVGA_ID_2 {
            return Err("VMware SVGA: SVGA_ID_2 negotiation failed");
        }

        let lfb_size = (width as usize) * (height as usize) * 4;
        let fb_virt = map_kernel_mmio(fb_bar.address, lfb_size)
            .map_err(|_| "VMware SVGA: failed to map Framebuffer")?;
        dev.framebuffer_virt = fb_virt.as_mut_ptr::<u32>();

        dev.write_reg(SVGA_REG_WIDTH, width);
        dev.write_reg(SVGA_REG_HEIGHT, height);
        dev.write_reg(SVGA_REG_BITS_PER_PIXEL, 32);
        dev.write_reg(SVGA_REG_ENABLE, 1);

        let pitch = dev.read_reg(SVGA_REG_BYTES_PER_LINE) / 4;
        if pitch > 0 {
            dev.pitch = pitch;
        }

        Ok(dev)
    }
}

impl DisplayDevice for VmwareSvgaDevice {
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
        self.write_reg(SVGA_REG_SYNC, 1);
        let _ = self.read_reg(SVGA_REG_BUSY);
        Ok(())
    }
}
