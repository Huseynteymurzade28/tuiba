//! Tiled background rendering: text backgrounds (modes 0–1) and
//! rotation/scaling backgrounds (modes 1–2).
//!
//! Each renderer fills a scanline buffer with 15-bit colours, using
//! [`TRANSPARENT`] where the layer shows nothing.

use crate::memory::VideoMemory;
use crate::memory::io::{IoRegisters, reg};
use crate::ppu::framebuffer::SCREEN_WIDTH;

/// Marker for "no pixel here": bit 15 is unused by BGR555.
pub const TRANSPARENT: u16 = 0x8000;

/// Size of one character (tile data) base block.
const CHAR_BLOCK: usize = 0x4000;
/// Size of one screen (tile map) base block.
const SCREEN_BLOCK: usize = 0x800;
/// Upper bound of background tile data in VRAM; object tiles live above.
const BG_VRAM_END: usize = 0x1_0000;

/// Decoded `MOSAIC`: block sizes in pixels (1 = off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mosaic {
    /// Background block width.
    pub bg_h: usize,
    /// Background block height.
    pub bg_v: usize,
    /// Object block width.
    pub obj_h: usize,
    /// Object block height.
    pub obj_v: usize,
}

impl Mosaic {
    /// Reads `MOSAIC`; each nibble stores the size minus one.
    #[must_use]
    pub fn read(io: &IoRegisters) -> Self {
        let v = io.read16(reg::MOSAIC);
        let size = |shift: u32| usize::from((v >> shift) & 0xF) + 1;
        Self {
            bg_h: size(0),
            bg_v: size(4),
            obj_h: size(8),
            obj_v: size(12),
        }
    }
}

/// Snaps `coord` to the top/left edge of its `size`-pixel mosaic block.
#[inline]
#[must_use]
pub const fn mosaic_snap(coord: usize, size: usize) -> usize {
    coord - coord % size
}

/// Applies horizontal mosaic to a rendered line: every pixel takes the
/// colour of the first pixel of its block.
pub fn mosaic_h(line: &mut [u16; SCREEN_WIDTH], size: usize) {
    if size <= 1 {
        return;
    }
    for block in line.chunks_mut(size) {
        let first = block[0];
        block.fill(first);
    }
}

/// Decoded `BGxCNT`.
#[derive(Debug, Clone, Copy)]
pub struct BgControl {
    /// Drawing priority, 0 (front) to 3 (back).
    pub priority: u8,
    /// Whether the mosaic effect applies to this background.
    pub mosaic: bool,
    /// Byte offset of tile data in VRAM.
    pub char_base: usize,
    /// Byte offset of the tile map in VRAM.
    pub screen_base: usize,
    /// 256-colour (8bpp) tiles; affine backgrounds are always 8bpp.
    pub color_256: bool,
    /// Affine only: wrap around instead of showing transparency outside.
    pub wrap: bool,
    /// Bits 15:14, interpreted per background type.
    pub size: u8,
}

impl BgControl {
    /// Reads and decodes `BGxCNT` for background `bg`.
    #[must_use]
    pub fn read(io: &IoRegisters, bg: usize) -> Self {
        let cnt = io.read16(reg::BG0CNT + 2 * bg as u32);
        Self {
            priority: (cnt & 0b11) as u8,
            mosaic: cnt & (1 << 6) != 0,
            char_base: usize::from((cnt >> 2) & 0b11) * CHAR_BLOCK,
            screen_base: usize::from((cnt >> 8) & 0x1F) * SCREEN_BLOCK,
            color_256: cnt & (1 << 7) != 0,
            wrap: cnt & (1 << 13) != 0,
            size: (cnt >> 14) as u8,
        }
    }
}

/// Reads background palette entry `index`.
#[inline]
fn bg_palette(video: &VideoMemory, index: usize) -> u16 {
    u16::from_le_bytes([video.palette[index * 2], video.palette[index * 2 + 1]])
}

/// Colour of pixel `(px, py)` inside 8bpp tile `tile`, or `TRANSPARENT`.
#[inline]
fn tile_pixel_8bpp(
    video: &VideoMemory,
    char_base: usize,
    tile: usize,
    px: usize,
    py: usize,
) -> u16 {
    let offset = char_base + tile * 64 + py * 8 + px;
    if offset >= BG_VRAM_END {
        return TRANSPARENT;
    }
    match video.vram[offset] {
        0 => TRANSPARENT,
        index => bg_palette(video, usize::from(index)),
    }
}

/// Colour of pixel `(px, py)` inside 4bpp tile `tile` using `palette`.
#[inline]
fn tile_pixel_4bpp(
    video: &VideoMemory,
    char_base: usize,
    tile: usize,
    palette: usize,
    px: usize,
    py: usize,
) -> u16 {
    let offset = char_base + tile * 32 + py * 4 + px / 2;
    if offset >= BG_VRAM_END {
        return TRANSPARENT;
    }
    let byte = video.vram[offset];
    let index = if px & 1 == 0 { byte & 0xF } else { byte >> 4 };
    if index == 0 {
        TRANSPARENT
    } else {
        bg_palette(video, palette * 16 + usize::from(index))
    }
}

/// Renders line `y` of text background `bg` into `out`.
pub fn render_text(
    io: &IoRegisters,
    video: &VideoMemory,
    bg: usize,
    y: usize,
    out: &mut [u16; SCREEN_WIDTH],
) {
    let cnt = BgControl::read(io, bg);
    let hofs = usize::from(io.read16(reg::BG0HOFS + 4 * bg as u32) & 0x1FF);
    let vofs = usize::from(io.read16(reg::BG0VOFS + 4 * bg as u32) & 0x1FF);
    let (width, height) = match cnt.size {
        0 => (256, 256),
        1 => (512, 256),
        2 => (256, 512),
        _ => (512, 512),
    };

    let yy = (y + vofs) & (height - 1);
    let (ty, py_base) = (yy / 8, yy & 7);
    // Screen blocks are laid out left-to-right, then top-to-bottom.
    let row_block = if yy >= 256 {
        if width == 512 { 2 } else { 1 }
    } else {
        0
    };
    let map_row = cnt.screen_base + (ty & 31) * 64;

    for (x, px_out) in out.iter_mut().enumerate() {
        let xx = (x + hofs) & (width - 1);
        let block = row_block + usize::from(xx >= 256);
        let entry_addr = map_row + block * SCREEN_BLOCK + (xx & 255) / 8 * 2;
        let entry = u16::from_le_bytes([video.vram[entry_addr], video.vram[entry_addr + 1]]);

        let tile = usize::from(entry & 0x3FF);
        let mut px = xx & 7;
        let mut py = py_base;
        if entry & (1 << 10) != 0 {
            px = 7 - px;
        }
        if entry & (1 << 11) != 0 {
            py = 7 - py;
        }

        *px_out = if cnt.color_256 {
            tile_pixel_8bpp(video, cnt.char_base, tile, px, py)
        } else {
            tile_pixel_4bpp(video, cnt.char_base, tile, usize::from(entry >> 12), px, py)
        };
    }
}

/// The affine parameters of BG2 or BG3.
#[derive(Debug, Clone, Copy)]
pub struct AffineParams {
    /// `dx` per screen pixel, 8.8 fixed point.
    pub pa: i32,
    /// `dx` per screen line.
    pub pb: i32,
    /// `dy` per screen pixel.
    pub pc: i32,
    /// `dy` per screen line.
    pub pd: i32,
    /// Reference point, 20.8 fixed point.
    pub x: i32,
    /// Reference point, 20.8 fixed point.
    pub y: i32,
}

impl AffineParams {
    /// Reads the parameters of background `bg` (2 or 3).
    #[must_use]
    pub fn read(io: &IoRegisters, bg: usize) -> Self {
        let base = if bg == 2 { reg::BG2PA } else { reg::BG3PA };
        let param = |offset| i32::from(io.read16(base + offset) as i16);
        // 28-bit signed reference points.
        let point = |offset| (io.read32(base + offset) << 4) as i32 >> 4;
        Self {
            pa: param(0),
            pb: param(2),
            pc: param(4),
            pd: param(6),
            x: point(8),
            y: point(12),
        }
    }
}

/// Renders line `y` of affine background `bg` into `out`.
pub fn render_affine(
    io: &IoRegisters,
    video: &VideoMemory,
    bg: usize,
    y: usize,
    out: &mut [u16; SCREEN_WIDTH],
) {
    let cnt = BgControl::read(io, bg);
    let params = AffineParams::read(io, bg);
    let size = 128usize << cnt.size;
    let tiles_per_row = size / 8;

    // Start of this line in texture space, then walk along PA/PC.
    let mut tx = params.x.wrapping_add(params.pb.wrapping_mul(y as i32));
    let mut ty = params.y.wrapping_add(params.pd.wrapping_mul(y as i32));

    for px_out in out.iter_mut() {
        let (mut x, mut y) = (tx >> 8, ty >> 8);
        tx = tx.wrapping_add(params.pa);
        ty = ty.wrapping_add(params.pc);

        if cnt.wrap {
            x &= size as i32 - 1;
            y &= size as i32 - 1;
        } else if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
            *px_out = TRANSPARENT;
            continue;
        }
        let (x, y) = (x as usize, y as usize);
        let tile = usize::from(video.vram[cnt.screen_base + (y / 8) * tiles_per_row + x / 8]);
        *px_out = tile_pixel_8bpp(video, cnt.char_base, tile, x & 7, y & 7);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Palette 1 colour 1 = red, 256-colour index 5 = green.
    fn setup() -> (IoRegisters, VideoMemory) {
        let mut video = VideoMemory::new();
        video.palette[(16 + 1) * 2..(16 + 1) * 2 + 2].copy_from_slice(&0x001Fu16.to_le_bytes());
        video.palette[5 * 2..5 * 2 + 2].copy_from_slice(&0x03E0u16.to_le_bytes());
        (IoRegisters::new(), video)
    }

    #[test]
    fn mosaic_register_and_horizontal_pass() {
        let (mut io, _) = setup();
        assert_eq!(
            Mosaic::read(&io),
            Mosaic {
                bg_h: 1,
                bg_v: 1,
                obj_h: 1,
                obj_v: 1
            }
        );
        io.write16(reg::MOSAIC, 0xF321);
        assert_eq!(
            Mosaic::read(&io),
            Mosaic {
                bg_h: 2,
                bg_v: 3,
                obj_h: 4,
                obj_v: 16
            }
        );
        assert_eq!(mosaic_snap(7, 3), 6);
        assert_eq!(mosaic_snap(7, 1), 7);

        let mut line: [u16; SCREEN_WIDTH] = std::array::from_fn(|x| x as u16);
        mosaic_h(&mut line, 1);
        assert_eq!(line[5], 5, "size 1 is a no-op");
        mosaic_h(&mut line, 7);
        assert_eq!(&line[..8], &[0, 0, 0, 0, 0, 0, 0, 7]);
        // 240 = 34 * 7 + 2: the last, partial block still snaps.
        assert_eq!(&line[238..], &[238, 238]);
    }

    #[test]
    fn text_bg_4bpp_with_palette_and_flip() {
        let (mut io, mut video) = setup();
        // BG0: char base 0, screen base block 1, 4bpp, 256x256.
        io.write16(reg::BG0CNT, 1 << 8);
        // Tile 1: only its top-left pixel (x=0,y=0) is colour 1.
        video.vram[32] = 0x01;
        // Map entry (0,0) = tile 1, palette 1 ; entry (1,0) = tile 1 hflip.
        let map = SCREEN_BLOCK;
        video.vram[map..map + 2].copy_from_slice(&(1u16 | 1 << 12).to_le_bytes());
        video.vram[map + 2..map + 4].copy_from_slice(&(1u16 | 1 << 12 | 1 << 10).to_le_bytes());

        let mut line = [0; SCREEN_WIDTH];
        render_text(&io, &video, 0, 0, &mut line);
        assert_eq!(line[0], 0x001F);
        assert_eq!(line[1], TRANSPARENT);
        assert_eq!(
            line[15], 0x001F,
            "flipped tile puts the pixel at x=7 of tile 2"
        );
        assert_eq!(line[8], TRANSPARENT);

        render_text(&io, &video, 0, 1, &mut line);
        assert_eq!(line[0], TRANSPARENT, "row 1 of the tile is empty");
    }

    #[test]
    fn text_bg_scrolls_and_wraps() {
        let (mut io, mut video) = setup();
        io.write16(reg::BG0CNT, 1 << 8);
        video.vram[32] = 0x01;
        let map = SCREEN_BLOCK;
        video.vram[map..map + 2].copy_from_slice(&(1u16 | 1 << 12).to_le_bytes());
        io.write16(reg::BG0HOFS, 255); // pixel 0 of the map appears at x = 1
        io.write16(reg::BG0VOFS, 256); // full wrap vertically
        let mut line = [0; SCREEN_WIDTH];
        render_text(&io, &video, 0, 0, &mut line);
        assert_eq!(line[1], 0x001F);
        assert_eq!(line[0], TRANSPARENT);
    }

    #[test]
    fn text_bg_8bpp_and_512_wide_map() {
        let (mut io, mut video) = setup();
        // 8bpp, 512x256: screen blocks 0 (left) and 1 (right); char base block 1.
        io.write16(reg::BG0CNT, (1 << 7) | (1 << 14) | (1 << 2));
        let char_base = CHAR_BLOCK;
        video.vram[char_base + 64 * 3 + 8 * 2 + 4] = 5; // tile 3, (4, 2) = index 5
        // Right screen block, entry (0, 0) = tile 3.
        video.vram[SCREEN_BLOCK..SCREEN_BLOCK + 2].copy_from_slice(&3u16.to_le_bytes());
        io.write16(reg::BG0HOFS, 256);
        let mut line = [0; SCREEN_WIDTH];
        render_text(&io, &video, 0, 2, &mut line);
        assert_eq!(line[4], 0x03E0);
        assert_eq!(line[3], TRANSPARENT);
    }

    #[test]
    fn affine_bg_identity_and_wrap() {
        let (mut io, mut video) = setup();
        // BG2: 8bpp tiles at char base 0, map at screen block 2, 128x128, wrap.
        io.write16(reg::BG2CNT, (2 << 8) | (1 << 13));
        io.write16(reg::BG2PA, 0x100);
        io.write16(reg::BG2PD, 0x100);
        video.vram[64 * 2 + 8 + 3] = 5; // tile 2, pixel (3, 1)
        video.vram[2 * SCREEN_BLOCK + 16 + 1] = 2; // map (1, 1) = tile 2
        let mut line = [0; SCREEN_WIDTH];
        render_affine(&io, &video, 2, 9, &mut line);
        assert_eq!(line[11], 0x03E0);
        assert_eq!(line[10], TRANSPARENT);
        assert_eq!(line[128 + 11], 0x03E0, "wraps at 128");

        io.write16(reg::BG2CNT, 2 << 8); // no wrap
        render_affine(&io, &video, 2, 9, &mut line);
        assert_eq!(line[128 + 11], TRANSPARENT);
        assert_eq!(line[11], 0x03E0);
    }

    #[test]
    fn affine_reference_point_and_scale() {
        let (mut io, mut video) = setup();
        io.write16(reg::BG2CNT, (2 << 8) | (1 << 13));
        io.write16(reg::BG2PA, 0x200); // 2x horizontal zoom-out
        io.write16(reg::BG2PD, 0x100);
        io.write32(reg::BG2X, 0xFFF_FF00 & 0x0FFF_FFFF); // -1.0 in 28-bit
        video.vram[64 * 2 + 3] = 5; // tile 2, pixel (3, 0)
        video.vram[2 * SCREEN_BLOCK] = 2; // map (0, 0) = tile 2
        let mut line = [0; SCREEN_WIDTH];
        render_affine(&io, &video, 2, 0, &mut line);
        // texture x = -1 + 2*sx -> pixel 3 at sx = 2
        assert_eq!(line[2], 0x03E0);
        assert_eq!(line[1], TRANSPARENT);
    }
}
