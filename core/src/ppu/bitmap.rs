//! Bitmap background modes 3, 4 and 5.
//!
//! These draw BG2 straight from VRAM with no tiles. They are what most
//! homebrew and test ROMs use, so they come first.

use crate::memory::VideoMemory;
use crate::ppu::framebuffer::{Rgba, SCREEN_WIDTH, bgr555_to_rgba};

/// Byte offset of the second frame in modes 4 and 5 (`DISPCNT` bit 4).
const FRAME1_OFFSET: usize = 0xA000;

/// Mode 3: 240×160, 16-bit colour directly in VRAM.
pub fn render_mode3(video: &VideoMemory, y: usize, out: &mut [Rgba]) {
    let base = y * SCREEN_WIDTH * 2;
    for (x, px) in out.iter_mut().enumerate() {
        let i = base + x * 2;
        *px = bgr555_to_rgba(u16::from_le_bytes([video.vram[i], video.vram[i + 1]]));
    }
}

/// Mode 4: 240×160, 8-bit palette indices, two frames.
pub fn render_mode4(video: &VideoMemory, frame1: bool, y: usize, out: &mut [Rgba]) {
    let base = if frame1 { FRAME1_OFFSET } else { 0 } + y * SCREEN_WIDTH;
    for (x, px) in out.iter_mut().enumerate() {
        *px = palette_color(video, video.vram[base + x]);
    }
}

/// Mode 5: 160×128, 16-bit colour, two frames. The area outside the
/// bitmap shows the backdrop.
pub fn render_mode5(video: &VideoMemory, frame1: bool, y: usize, out: &mut [Rgba]) {
    const W: usize = 160;
    const H: usize = 128;
    let backdrop = palette_color(video, 0);
    if y >= H {
        out.fill(backdrop);
        return;
    }
    let base = if frame1 { FRAME1_OFFSET } else { 0 } + y * W * 2;
    for (x, px) in out.iter_mut().enumerate() {
        *px = if x < W {
            let i = base + x * 2;
            bgr555_to_rgba(u16::from_le_bytes([video.vram[i], video.vram[i + 1]]))
        } else {
            backdrop
        };
    }
}

/// Looks up entry `index` of the background palette.
#[inline]
#[must_use]
pub fn palette_color(video: &VideoMemory, index: u8) -> Rgba {
    let i = usize::from(index) * 2;
    bgr555_to_rgba(u16::from_le_bytes([video.palette[i], video.palette[i + 1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> VideoMemory {
        let mut v = VideoMemory::new();
        v.palette[0..2].copy_from_slice(&0x7C00u16.to_le_bytes()); // backdrop blue
        v.palette[2..4].copy_from_slice(&0x03E0u16.to_le_bytes()); // index 1 green
        v
    }

    #[test]
    fn mode3_reads_16bit_pixels() {
        let mut v = video();
        let i = (5 * SCREEN_WIDTH + 7) * 2;
        v.vram[i..i + 2].copy_from_slice(&0x001Fu16.to_le_bytes());
        let mut row = [0; SCREEN_WIDTH];
        render_mode3(&v, 5, &mut row);
        assert_eq!(row[7], 0xFF00_00FF);
        assert_eq!(row[8], 0x0000_00FF);
    }

    #[test]
    fn mode4_uses_palette_and_frames() {
        let mut v = video();
        v.vram[3 * SCREEN_WIDTH + 1] = 1;
        v.vram[FRAME1_OFFSET + 3 * SCREEN_WIDTH + 2] = 1;
        let mut row = [0; SCREEN_WIDTH];
        render_mode4(&v, false, 3, &mut row);
        assert_eq!(row[1], 0x00FF_00FF);
        assert_eq!(row[2], 0x0000_FFFF, "backdrop");
        render_mode4(&v, true, 3, &mut row);
        assert_eq!(row[1], 0x0000_FFFF);
        assert_eq!(row[2], 0x00FF_00FF);
    }

    #[test]
    fn mode5_letterboxes_with_backdrop() {
        let mut v = video();
        v.vram[0..2].copy_from_slice(&0x7FFFu16.to_le_bytes());
        let mut row = [0; SCREEN_WIDTH];
        render_mode5(&v, false, 0, &mut row);
        assert_eq!(row[0], 0xFFFF_FFFF);
        assert_eq!(row[159], 0x0000_00FF);
        assert_eq!(row[160], 0x0000_FFFF, "right of bitmap");
        render_mode5(&v, false, 128, &mut row);
        assert_eq!(row[0], 0x0000_FFFF, "below bitmap");
    }
}
