use x86_64::instructions::port::Port;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{DisplayDevice, DisplayDeviceError};
use crate::memory::map_kernel_mmio;

pub const BOCHS_PCI_VENDOR_ID: u16 = 0x1234;
pub const BOCHS_VGA_DEVICE_ID: u16 = 0x1111;

const VBE_DISPI_IOPORT_INDEX: u16 = 0x01CE;
const VBE_DISPI_IOPORT_DATA: u16 = 0x01CF;

const VBE_DISPI_INDEX_XRES: u16 = 0x1;
const VBE_DISPI_INDEX_YRES: u16 = 0x2;
const VBE_DISPI_INDEX_BPP: u16 = 0x3;
const VBE_DISPI_INDEX_ENABLE: u16 = 0x4;
const VBE_DISPI_INDEX_BANK: u16 = 0x5;

const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;

/// Драйвер стандартного видеоадаптера Bochs/QEMU VGA (QEMU `-vga std`, Proxmox Standard VGA).
/// Управляет видеорежимом через стандартные порты VBE DISPI 0x1CE/0x1CF и мапит линейный фреймбуфер LFB.
pub struct BochsVgaDevice {
    width: u32,
    height: u32,
    pitch: u32,
    framebuffer_virt: *mut u32,
}

unsafe impl Send for BochsVgaDevice {}
unsafe impl Sync for BochsVgaDevice {}

impl BochsVgaDevice {
    fn write_reg(index: u16, value: u16) {
        unsafe {
            let mut idx_port = Port::<u16>::new(VBE_DISPI_IOPORT_INDEX);
            let mut data_port = Port::<u16>::new(VBE_DISPI_IOPORT_DATA);
            idx_port.write(index);
            data_port.write(value);
        }
    }

    fn read_reg(index: u16) -> u16 {
        unsafe {
            let mut idx_port = Port::<u16>::new(VBE_DISPI_IOPORT_INDEX);
            let mut data_port = Port::<u16>::new(VBE_DISPI_IOPORT_DATA);
            idx_port.write(index);
            data_port.read()
        }
    }

    pub fn new(pci: &PciDevice, width: u32, height: u32) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_io_space();

        // Проверяем, активен ли уже графический VBE режим, настроенный загрузчиком Limine
        let is_vbe_active = (Self::read_reg(VBE_DISPI_INDEX_ENABLE) & VBE_DISPI_ENABLED) != 0;
        let (real_w, real_h) = if is_vbe_active {
            let cur_w = Self::read_reg(VBE_DISPI_INDEX_XRES) as u32;
            let cur_h = Self::read_reg(VBE_DISPI_INDEX_YRES) as u32;
            if cur_w > 0 && cur_h > 0 {
                (cur_w, cur_h)
            } else {
                (width, height)
            }
        } else {
            // Если режим еще не включен, задаем его без отключения
            Self::write_reg(VBE_DISPI_INDEX_XRES, width as u16);
            Self::write_reg(VBE_DISPI_INDEX_YRES, height as u16);
            Self::write_reg(VBE_DISPI_INDEX_BPP, 32);
            Self::write_reg(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED);
            Self::write_reg(VBE_DISPI_INDEX_BANK, 0);
            (width, height)
        };

        let lfb_bar = pci.first_memory_bar().ok_or("bochs-vga: BAR0 LFB not found")?;
        let lfb_size = (real_w as usize) * (real_h as usize) * 4;

        let virt_addr = map_kernel_mmio(lfb_bar.address, lfb_size)
            .map_err(|_| "bochs-vga: failed to map LFB MMIO")?;

        let fb_ptr = virt_addr.as_mut_ptr::<u32>();

        Ok(Self {
            width: real_w,
            height: real_h,
            pitch: real_w,
            framebuffer_virt: fb_ptr,
        })
    }
}

impl DisplayDevice for BochsVgaDevice {
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
