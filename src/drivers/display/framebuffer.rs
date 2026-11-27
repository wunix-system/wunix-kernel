pub const DEFAULT_FG_COLOR: u32 = 0xFFFFFF;
pub const DEFAULT_BG_COLOR: u32 = 0x000000;

// Отступы терминала от краев экрана
pub const MARGIN_X: usize = 12;
pub const MARGIN_Y: usize = 10;

#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub bpp: u16,
}

unsafe impl Send for Framebuffer {}
unsafe impl Sync for Framebuffer {}

impl Framebuffer {
    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            let offset = y * self.pitch + x * 4;
            unsafe {
                core::ptr::write_volatile(self.address.add(offset) as *mut u32, color);
            }
        }
    }

    pub fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, color: u32) {
        let xe = x1.min(self.width);
        let ye = y1.min(self.height);
        for y in y0..ye {
            for x in x0..xe {
                self.put_pixel(x, y, color);
            }
        }
    }

    pub fn clear(&mut self, color: u32) {
        if color == 0 {
            let total_bytes = self.height * self.pitch;
            unsafe {
                core::ptr::write_bytes(self.address, 0, total_bytes);
            }
        } else {
            self.fill_rect(0, 0, self.width, self.height, color);
        }
    }
}
