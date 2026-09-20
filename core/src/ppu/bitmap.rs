//! Bitmap background modes 3, 4 and 5.
//!
//! These draw BG2 straight from VRAM with no tiles, producing a 15-bit
//! scanline like the tiled renderers so the compositor treats them alike.

use crate::memory::VideoMemory;
use crate::ppu::framebuffer::SCREEN_WIDTH;
use crate::ppu::tiled::TRANSPARENT;

/// Byte offset of the second frame in modes 4 and 5 (`DISPCNT` bit 4).
const FRAME1_OFFSET: usize = 0xA000;

#[inline]
fn vram16(video: &VideoMemory, offset: usize) -> u16 {
    u16::from_le_bytes([video.vram[offset], video.vram[offset + 1]])
}

/// Mode 3: 240×160, 16-bit colour directly in VRAM. Always opaque.
pub fn render_mode3(video: &VideoMemory, y: usize, out: &mut [u16; SCREEN_WIDTH]) {
    let base = y * SCREEN_WIDTH * 2;
    for (x, px) in out.iter_mut().enumerate() {
        *px = vram16(video, base + x * 2) & 0x7FFF;
    }
}

/// Mode 4: 240×160, 8-bit palette indices, two frames. Index 0 is
/// transparent.
pub fn render_mode4(video: &VideoMemory, frame1: bool, y: usize, out: &mut [u16; SCREEN_WIDTH]) {
    let base = if frame1 { FRAME1_OFFSET } else { 0 } + y * SCREEN_WIDTH;
    for (x, px) in out.iter_mut().enumerate() {
        *px = match video.vram[base + x] {
            0 => TRANSPARENT,
            index => palette_entry(video, usize::from(index)),
        };
    }
}

/// Mode 5: 160×128, 16-bit colour, two frames. Outside the bitmap the
/// layer is transparent.
pub fn render_mode5(video: &VideoMemory, frame1: bool, y: usize, out: &mut [u16; SCREEN_WIDTH]) {
    const W: usize = 160;
    const H: usize = 128;
    if y >= H {
        out.fill(TRANSPARENT);
        return;
    }
    let base = if frame1 { FRAME1_OFFSET } else { 0 } + y * W * 2;
    for (x, px) in out.iter_mut().enumerate() {
        *px = if x < W {
            vram16(video, base + x * 2) & 0x7FFF
        } else {
            TRANSPARENT
        };
    }
}

/// Entry `index` of the background palette as a 15-bit colour.
#[inline]
#[must_use]
pub fn palette_entry(video: &VideoMemory, index: usize) -> u16 {
    u16::from_le_bytes([video.palette[index * 2], video.palette[index * 2 + 1]]) & 0x7FFF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> VideoMemory {
        let mut v = VideoMemory::new();
        v.palette[2..4].copy_from_slice(&0x03E0u16.to_le_bytes()); // index 1 green
        v
    }

    #[test]
    fn mode3_reads_16bit_pixels() {
        let mut v = video();
        let i = (5 * SCREEN_WIDTH + 7) * 2;
        v.vram[i..i + 2].copy_from_slice(&0x801Fu16.to_le_bytes());
        let mut row = [0; SCREEN_WIDTH];
        render_mode3(&v, 5, &mut row);
        assert_eq!(row[7], 0x001F, "bit 15 stripped");
        assert_eq!(row[8], 0x0000, "black, not transparent");
    }

    #[test]
    fn mode4_uses_palette_and_frames() {
        let mut v = video();
        v.vram[3 * SCREEN_WIDTH + 1] = 1;
        v.vram[FRAME1_OFFSET + 3 * SCREEN_WIDTH + 2] = 1;
        let mut row = [0; SCREEN_WIDTH];
        render_mode4(&v, false, 3, &mut row);
        assert_eq!(row[1], 0x03E0);
        assert_eq!(row[2], TRANSPARENT);
        render_mode4(&v, true, 3, &mut row);
        assert_eq!(row[1], TRANSPARENT);
        assert_eq!(row[2], 0x03E0);
    }

    #[test]
    fn mode5_is_transparent_outside_bitmap() {
        let mut v = video();
        v.vram[0..2].copy_from_slice(&0x7FFFu16.to_le_bytes());
        let mut row = [0; SCREEN_WIDTH];
        render_mode5(&v, false, 0, &mut row);
        assert_eq!(row[0], 0x7FFF);
        assert_eq!(row[159], 0);
        assert_eq!(row[160], TRANSPARENT);
        render_mode5(&v, false, 128, &mut row);
        assert_eq!(row[0], TRANSPARENT);
    }
}
