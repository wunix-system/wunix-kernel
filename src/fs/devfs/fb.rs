use super::types::CharDeviceOp;
use crate::fs::vfs::VfsError;

pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FbBitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FbVarScreeninfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: FbBitfield,
    pub green: FbBitfield,
    pub blue: FbBitfield,
    pub transp: FbBitfield,
    pub nonstd: u32,
    pub activate: u32,
    pub height: u32,
    pub width: u32,
    pub accel_flags: u32,
    pub pixclock: u32,
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    pub sync: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FbFixScreeninfo {
    pub id: [u8; 16],
    pub smem_start: u64,
    pub smem_len: u32,
    pub type_: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: u64,
    pub mmio_len: u32,
    pub accel: u32,
    pub capabilities: u16,
    pub reserved: [u16; 2],
}

/// /dev/fb0: аппаратный кадровый буфер графического дисплея (Linux Framebuffer Device)
pub struct FbDev;

impl FbDev {
    fn fb_info(&self) -> Option<(usize, usize, usize, u16, *mut u8)> {
        let guard = crate::drivers::display::CONSOLE.lock();
        let console = guard.as_ref()?;
        let fb = &console.framebuffer;
        Some((fb.width, fb.height, fb.pitch, fb.bpp, fb.address))
    }
}

impl CharDeviceOp for FbDev {
    fn size(&self) -> usize {
        if let Some((_, height, pitch, _, _)) = self.fb_info() {
            height * pitch
        } else {
            0
        }
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let total = self.size();
        if offset >= total || buf.is_empty() {
            return Ok(0);
        }
        let (_, _, _, _, addr) = self.fb_info().ok_or(VfsError::IoError)?;
        let to_read = buf.len().min(total - offset);
        unsafe {
            let src = addr.add(offset);
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), to_read);
        }
        Ok(to_read)
    }

    fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let total = self.size();
        if offset >= total || buf.is_empty() {
            return Ok(0);
        }
        let (_, _, _, _, addr) = self.fb_info().ok_or(VfsError::IoError)?;
        let to_write = buf.len().min(total - offset);
        unsafe {
            let dst = addr.add(offset);
            core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, to_write);
        }
        Ok(to_write)
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        let (width, height, pitch, bpp, addr) = self.fb_info().ok_or(-5i64)?;

        match cmd {
            FBIOGET_VSCREENINFO => {
                let sz = core::mem::size_of::<FbVarScreeninfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let mut vinfo = FbVarScreeninfo::default();
                vinfo.xres = width as u32;
                vinfo.yres = height as u32;
                vinfo.xres_virtual = width as u32;
                vinfo.yres_virtual = height as u32;
                vinfo.bits_per_pixel = bpp as u32;
                vinfo.red = FbBitfield { offset: 16, length: 8, msb_right: 0 };
                vinfo.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
                vinfo.blue = FbBitfield { offset: 0, length: 8, msb_right: 0 };
                vinfo.transp = FbBitfield { offset: 24, length: 8, msb_right: 0 };
                unsafe {
                    core::ptr::write_unaligned(arg as *mut FbVarScreeninfo, vinfo);
                }
                Ok(0)
            }
            FBIOPUT_VSCREENINFO => {
                let sz = core::mem::size_of::<FbVarScreeninfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                Ok(0)
            }
            FBIOGET_FSCREENINFO => {
                let sz = core::mem::size_of::<FbFixScreeninfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let mut finfo = FbFixScreeninfo::default();
                let name = b"Wunix FB\0";
                finfo.id[..name.len()].copy_from_slice(name);
                finfo.smem_start = addr as u64;
                finfo.smem_len = (height * pitch) as u32;
                finfo.type_ = 0; // FB_TYPE_PACKED_PIXELS
                finfo.visual = 2; // FB_VISUAL_TRUECOLOR
                finfo.line_length = pitch as u32;
                unsafe {
                    core::ptr::write_unaligned(arg as *mut FbFixScreeninfo, finfo);
                }
                Ok(0)
            }
            _ => Err(-25),
        }
    }
}
