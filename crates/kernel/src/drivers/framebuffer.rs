#[derive(Debug, Clone, Copy)]
pub enum PixelFormat {
    Rgb,
    Bgr,
}

pub struct FramebufferInfo {
    pub base: *mut u8,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixel_format: PixelFormat,
}

impl FramebufferInfo {
    pub(crate) unsafe fn write_pixel(&self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.stride + x) * 4;
        if idx + 3 >= self.size {
            return;
        }
        unsafe {
            let ptr = self.base.add(idx);
            match self.pixel_format {
                PixelFormat::Bgr => {
                    ptr.add(0).write_volatile(b);
                    ptr.add(1).write_volatile(g);
                    ptr.add(2).write_volatile(r);
                    ptr.add(3).write_volatile(0);
                }
                PixelFormat::Rgb => {
                    ptr.add(0).write_volatile(r);
                    ptr.add(1).write_volatile(g);
                    ptr.add(2).write_volatile(b);
                    ptr.add(3).write_volatile(0);
                }
            }
        }
    }

    pub(crate) unsafe fn blend_pixel(&self, x: usize, y: usize, r: u8, g: u8, b: u8, alpha: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.stride + x) * 4;
        if idx + 3 >= self.size {
            return;
        }
        unsafe {
            let ptr = self.base.add(idx);
            match self.pixel_format {
                PixelFormat::Bgr => {
                    let old_b = ptr.add(0).read_volatile();
                    let old_g = ptr.add(1).read_volatile();
                    let old_r = ptr.add(2).read_volatile();
                    let a = alpha as u16;
                    let new_r = ((r as u16 * a + old_r as u16 * (255 - a)) / 255) as u8;
                    let new_g = ((g as u16 * a + old_g as u16 * (255 - a)) / 255) as u8;
                    let new_b = ((b as u16 * a + old_b as u16 * (255 - a)) / 255) as u8;
                    ptr.add(0).write_volatile(new_b);
                    ptr.add(1).write_volatile(new_g);
                    ptr.add(2).write_volatile(new_r);
                }
                PixelFormat::Rgb => {
                    let old_r = ptr.add(0).read_volatile();
                    let old_g = ptr.add(1).read_volatile();
                    let old_b = ptr.add(2).read_volatile();
                    let a = alpha as u16;
                    let new_r = ((r as u16 * a + old_r as u16 * (255 - a)) / 255) as u8;
                    let new_g = ((g as u16 * a + old_g as u16 * (255 - a)) / 255) as u8;
                    let new_b = ((b as u16 * a + old_b as u16 * (255 - a)) / 255) as u8;
                    ptr.add(0).write_volatile(new_r);
                    ptr.add(1).write_volatile(new_g);
                    ptr.add(2).write_volatile(new_b);
                }
            }
        }
    }

    pub(crate) unsafe fn scroll_up(&self, pixels: usize) {
        let row_bytes = self.stride * 4;
        let scroll_bytes = pixels * row_bytes;
        let visible_bytes = (self.height.saturating_sub(pixels)) * row_bytes;
        unsafe {
            if visible_bytes > 0 {
                core::ptr::copy(self.base.add(scroll_bytes), self.base, visible_bytes);
            }
            let clear_start = visible_bytes.min(self.size);
            let clear_bytes = self.size - clear_start;
            core::ptr::write_bytes(self.base.add(clear_start), 0, clear_bytes);
        }
    }

    pub(crate) unsafe fn clear(&self, r: u8, g: u8, b: u8) {
        for y in 0..self.height {
            for x in 0..self.width {
                unsafe {
                    self.write_pixel(x, y, r, g, b);
                }
            }
        }
    }
}
