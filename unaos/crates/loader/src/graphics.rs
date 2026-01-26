use uefi::proto::console::gop::PixelFormat;
use crate::font::FONT;

pub struct FramebufferWriter {
    pub base_addr: *mut u8,
    pub stride: usize,
    pub format: PixelFormat,
    pub bytes_per_pixel: usize,
}

impl FramebufferWriter {
    pub fn new(base_addr: *mut u8, stride: usize, format: PixelFormat, bytes_per_pixel: usize) -> Self {
        Self {
            base_addr,
            stride,
            format,
            bytes_per_pixel,
        }
    }

    /// Draws a pixel at the given coordinates with the specified color.
    /// The color is provided as [R, G, B].
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: [u8; 3]) {
        let pixel_offset = (y * self.stride + x) * self.bytes_per_pixel;

        unsafe {
            let pixel_ptr = self.base_addr.add(pixel_offset);
            match self.format {
                PixelFormat::Rgb => {
                    // RGB: [R, G, B]
                    pixel_ptr.write_volatile(color[0]);
                    pixel_ptr.add(1).write_volatile(color[1]);
                    pixel_ptr.add(2).write_volatile(color[2]);
                },
                PixelFormat::Bgr => {
                    // BGR: [B, G, R]
                    pixel_ptr.write_volatile(color[2]);
                    pixel_ptr.add(1).write_volatile(color[1]);
                    pixel_ptr.add(2).write_volatile(color[0]);
                },
                _ => {
                    // Fallback for other formats (e.g. Bitmask) - use Grey for visibility if unknown
                    // Ideally we should handle Bitmask properly, but for this task Rgb/Bgr are primary.
                    pixel_ptr.write_volatile(128);
                    pixel_ptr.add(1).write_volatile(128);
                    pixel_ptr.add(2).write_volatile(128);
                }
            }
        }
    }

    /// Draws a character at the given coordinates.
    pub fn draw_char(&mut self, x: usize, y: usize, c: char, color: [u8; 3]) {
        let char_index = c as usize;
        let bitmap = if char_index < 128 {
            FONT[char_index]
        } else {
            FONT[0x3F] // Use '?' for unknown characters (or maybe a block)
        };

        for row in 0..8 {
            for col in 0..8 {
                // Polarity Inversion: The font data is LSB-Left.
                // We read from bit 0 (LSB) to bit 7 (MSB), mapping LSB to column 0.
                if (bitmap[row] >> col) & 1 == 1 {
                    self.draw_pixel(x + col, y + row, color);
                }
            }
        }
    }

    /// Draws a string starting at the given coordinates.
    pub fn draw_string(&mut self, mut x: usize, y: usize, s: &str, color: [u8; 3]) {
        for c in s.chars() {
            self.draw_char(x, y, c, color);
            x += 8;
        }
    }
}
