//! LCD colour correction.
//!
//! The GBA's screen is dim, its gamma steep and its primaries bleed into
//! each other, so games were coloured to look right on it; the same
//! values shown as plain sRGB come out oversaturated and too bright.
//! [`correct`] maps each pixel through the approximation of that screen
//! used by higan and ares (by Talarabi and byuu): decode with the LCD's
//! gamma of 4, mix the channels, encode with a display gamma of 2.2.
//!
//! The core's framebuffer stays the raw colours; this is a view of it.

use std::sync::OnceLock;

use tuiba_core::Framebuffer;
use tuiba_core::ppu::framebuffer::Rgba;

/// Gamma of the GBA's LCD.
const LCD_GAMMA: f32 = 4.0;
/// Gamma of the display the result is shown on.
const OUT_GAMMA: f32 = 2.2;

/// Writes `src` with the LCD's colours into `dst`.
pub fn correct(src: &Framebuffer, dst: &mut Framebuffer) {
    let table = table();
    for y in 0..tuiba_core::SCREEN_HEIGHT {
        for (out, &p) in dst.row_mut(y).iter_mut().zip(src.row(y)) {
            *out = table[index(p)];
        }
    }
}

/// The 15-bit colour an expanded pixel came from: the top five bits of
/// each channel.
const fn index(p: Rgba) -> usize {
    let r = (p >> 27) & 0x1F;
    let g = (p >> 19) & 0x1F;
    let b = (p >> 11) & 0x1F;
    (r | (g << 5) | (b << 10)) as usize
}

/// Every 15-bit colour, corrected; built on first use.
fn table() -> &'static [Rgba] {
    static TABLE: OnceLock<Box<[Rgba]>> = OnceLock::new();
    TABLE.get_or_init(|| (0..0x8000u32).map(corrected).collect())
}

/// The colour the LCD shows for the 15-bit `bgr`.
fn corrected(bgr: u32) -> Rgba {
    let linear = |shift: u32| (f32::from((bgr >> shift) as u8 & 0x1F) / 31.0).powf(LCD_GAMMA);
    let (r, g, b) = (linear(0), linear(5), linear(10));
    // The rows sum to 305, 270 and 280, so full white would overshoot;
    // scaling by 255/280 brings it just under (a faint warm tint, as on
    // the hardware).
    let encode = |mix: f32| {
        let v = (mix / 255.0).powf(1.0 / OUT_GAMMA) * (255.0 / 280.0);
        (v.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    let out_r = encode(255.0 * r + 50.0 * g);
    let out_g = encode(10.0 * r + 230.0 * g + 30.0 * b);
    let out_b = encode(50.0 * r + 10.0 * g + 220.0 * b);
    (out_r << 24) | (out_g << 16) | (out_b << 8) | 0xFF
}

#[cfg(test)]
mod tests {
    use tuiba_core::ppu::framebuffer::bgr555_to_rgba;

    use super::*;

    #[test]
    fn index_inverts_the_expansion() {
        for bgr in 0..0x8000u16 {
            assert_eq!(index(bgr555_to_rgba(bgr)), usize::from(bgr));
        }
    }

    #[test]
    fn black_stays_black_and_colours_are_muted() {
        assert_eq!(corrected(0), 0x0000_00FF);
        let white = corrected(0x7FFF);
        let channel = |p: Rgba, shift: u32| (p >> shift) & 0xFF;
        assert!(channel(white, 24) > 0xF0 && channel(white, 16) > 0xE0);
        // Pure blue picks up some red and green and loses some of itself.
        let blue = corrected(0x7C00);
        assert!(channel(blue, 8) < 0xFF);
        assert!(channel(blue, 16) > 0 && channel(blue, 24) == 0);
        // A mid grey comes out darker: the LCD's gamma is steeper.
        let grey = corrected(0x3DEF); // 15 in every channel
        assert!(channel(grey, 16) < 0x7B);
    }

    #[test]
    fn corrects_a_whole_frame() {
        let mut src = Framebuffer::new();
        src.row_mut(5)[7] = bgr555_to_rgba(0x001F);
        let mut dst = Framebuffer::new();
        correct(&src, &mut dst);
        assert_eq!(dst.row(5)[7], corrected(0x001F));
        assert_eq!(dst.row(0)[0], corrected(0));
    }
}
