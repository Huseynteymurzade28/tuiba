//! The 240×160 output image.

/// Screen width in pixels.
pub const SCREEN_WIDTH: usize = 240;
/// Screen height in pixels.
pub const SCREEN_HEIGHT: usize = 160;

/// A packed `0xRRGGBBAA` pixel.
pub type Rgba = u32;

/// Converts a GBA 15-bit `xBBBBBGGGGGRRRRR` colour to opaque RGBA8.
///
/// Each 5-bit channel is expanded to 8 bits by replicating its top bits
/// into the low bits, so `0x1F` maps to `0xFF` rather than `0xF8`.
#[inline]
#[must_use]
pub const fn bgr555_to_rgba(color: u16) -> Rgba {
    let r = expand5(color);
    let g = expand5(color >> 5);
    let b = expand5(color >> 10);
    (r << 24) | (g << 16) | (b << 8) | 0xFF
}

/// Expands the low 5 bits of `c` to 8 bits.
#[inline]
const fn expand5(c: u16) -> u32 {
    let c = (c & 0x1F) as u32;
    (c << 3) | (c >> 2)
}

/// The rendered frame, one [`Rgba`] per pixel in row-major order.
///
/// The frontend borrows this between frames; it is never copied.
#[derive(Debug, Clone)]
pub struct Framebuffer {
    pixels: Box<[Rgba]>,
}

impl Default for Framebuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framebuffer {
    /// A black, opaque frame.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pixels: vec![0x0000_00FF; SCREEN_WIDTH * SCREEN_HEIGHT].into_boxed_slice(),
        }
    }

    /// All pixels, row-major, `SCREEN_WIDTH * SCREEN_HEIGHT` long.
    #[inline]
    #[must_use]
    pub fn pixels(&self) -> &[Rgba] {
        &self.pixels
    }

    /// One scanline.
    #[inline]
    #[must_use]
    pub fn row(&self, y: usize) -> &[Rgba] {
        &self.pixels[y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH]
    }

    /// Mutable access to one scanline, for the renderer.
    #[inline]
    pub(crate) fn row_mut(&mut self, y: usize) -> &mut [Rgba] {
        &mut self.pixels[y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH]
    }

    /// The `(r, g, b)` of a pixel.
    #[inline]
    #[must_use]
    pub fn rgb(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let p = self.pixels[y * SCREEN_WIDTH + x];
        ((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_conversion_uses_full_range() {
        assert_eq!(bgr555_to_rgba(0x0000), 0x0000_00FF);
        assert_eq!(bgr555_to_rgba(0x7FFF), 0xFFFF_FFFF);
        assert_eq!(bgr555_to_rgba(0x001F), 0xFF00_00FF, "red");
        assert_eq!(bgr555_to_rgba(0x03E0), 0x00FF_00FF, "green");
        assert_eq!(bgr555_to_rgba(0x7C00), 0x0000_FFFF, "blue");
        assert_eq!(bgr555_to_rgba(0x8000), 0x0000_00FF, "bit 15 ignored");
        assert_eq!(bgr555_to_rgba(0x0010), 0x8400_00FF, "mid red = 0x84");
    }

    #[test]
    fn framebuffer_rows_and_rgb() {
        let mut fb = Framebuffer::new();
        fb.row_mut(1)[2] = 0x1122_33FF;
        assert_eq!(fb.row(1)[2], 0x1122_33FF);
        assert_eq!(fb.pixels()[SCREEN_WIDTH + 2], 0x1122_33FF);
        assert_eq!(fb.rgb(2, 1), (0x11, 0x22, 0x33));
        assert_eq!(fb.rgb(0, 0), (0, 0, 0));
    }
}
