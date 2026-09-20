//! Sprite (OBJ) rendering from OAM.
//!
//! All 128 objects are scanned per line. The OBJ layer keeps, per pixel,
//! the first opaque sprite in OAM order together with its priority;
//! among sprites OAM order wins outright, the priority attribute only
//! matters against backgrounds (this is the real hardware behaviour).

use crate::memory::VideoMemory;
use crate::memory::io::{IoRegisters, reg};
use crate::ppu::framebuffer::SCREEN_WIDTH;
use crate::ppu::tiled::TRANSPARENT;

/// Start of object tile data in VRAM.
const OBJ_VRAM_BASE: usize = 0x1_0000;
/// Object palette offset in palette RAM.
const OBJ_PALETTE_BASE: usize = 0x200;
/// Number of OAM entries.
const OBJ_COUNT: usize = 128;

/// `(width, height)` for each `(shape, size)` combination.
const SIZES: [[(usize, usize); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

/// A decoded OAM entry.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // mirrors the attribute bits
struct Object {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    affine: Option<usize>,
    double_size: bool,
    hflip: bool,
    vflip: bool,
    color_256: bool,
    tile: usize,
    priority: u8,
    palette: usize,
    /// Mode 2 (object window) contributes no pixels.
    hidden: bool,
}

impl Object {
    fn read(video: &VideoMemory, index: usize) -> Option<Self> {
        let base = index * 8;
        let attr =
            |i: usize| u16::from_le_bytes([video.oam[base + i * 2], video.oam[base + i * 2 + 1]]);
        let (a0, a1, a2) = (attr(0), attr(1), attr(2));

        let rotation = a0 & (1 << 8) != 0;
        if !rotation && a0 & (1 << 9) != 0 {
            return None; // disabled
        }
        let shape = usize::from(a0 >> 14);
        if shape == 3 {
            return None; // prohibited
        }
        let (width, height) = SIZES[shape][usize::from(a1 >> 14)];

        // Y is 8-bit; objects that would extend past the bottom wrap to
        // the top, so treat large values as negative.
        let mut y = i32::from(a0 & 0xFF);
        let box_height = if rotation && a0 & (1 << 9) != 0 {
            height * 2
        } else {
            height
        };
        if y + box_height as i32 > 256 {
            y -= 256;
        }
        // X is 9-bit signed.
        let x = (i32::from(a1 & 0x1FF) << 23) >> 23;

        Some(Self {
            x,
            y,
            width,
            height,
            affine: rotation.then(|| usize::from((a1 >> 9) & 0x1F)),
            double_size: rotation && a0 & (1 << 9) != 0,
            hflip: !rotation && a1 & (1 << 12) != 0,
            vflip: !rotation && a1 & (1 << 13) != 0,
            color_256: a0 & (1 << 13) != 0,
            tile: usize::from(a2 & 0x3FF),
            priority: ((a2 >> 10) & 0b11) as u8,
            palette: usize::from(a2 >> 12),
            hidden: (a0 >> 10) & 0b11 == 2,
        })
    }

    /// Screen-space bounding box size.
    fn box_size(&self) -> (usize, usize) {
        if self.double_size {
            (self.width * 2, self.height * 2)
        } else {
            (self.width, self.height)
        }
    }
}

/// Reads the affine matrix stored in OAM group `index`.
fn affine_params(video: &VideoMemory, index: usize) -> [i32; 4] {
    let base = index * 32 + 6;
    std::array::from_fn(|i| {
        let o = base + i * 8;
        i32::from(i16::from_le_bytes([video.oam[o], video.oam[o + 1]]))
    })
}

/// Colour of texel `(tx, ty)` of `obj`, or `TRANSPARENT`.
#[inline]
fn texel(
    video: &VideoMemory,
    obj: &Object,
    one_dimensional: bool,
    bitmap_mode: bool,
    tx: usize,
    ty: usize,
) -> u16 {
    let (tile_x, tile_y) = (tx / 8, ty / 8);
    // Tile numbers count 32-byte units; 8bpp tiles occupy two.
    let per_tile = if obj.color_256 { 2 } else { 1 };
    let row_stride = if one_dimensional {
        obj.width / 8 * per_tile
    } else {
        32
    };
    let tile = obj.tile + tile_y * row_stride + tile_x * per_tile;
    // Bitmap modes use the lower half of object VRAM for the frame.
    if bitmap_mode && tile < 512 {
        return TRANSPARENT;
    }
    let offset = OBJ_VRAM_BASE + (tile & 0x3FF) * 32;
    let (px, py) = (tx & 7, ty & 7);

    let index = if obj.color_256 {
        video.vram[(offset + py * 8 + px) & 0x1_FFFF]
    } else {
        let byte = video.vram[(offset + py * 4 + px / 2) & 0x1_FFFF];
        if px & 1 == 0 { byte & 0xF } else { byte >> 4 }
    };
    if index == 0 {
        return TRANSPARENT;
    }
    let entry = if obj.color_256 {
        usize::from(index)
    } else {
        obj.palette * 16 + usize::from(index)
    };
    let p = OBJ_PALETTE_BASE + entry * 2;
    u16::from_le_bytes([video.palette[p], video.palette[p + 1]]) & 0x7FFF
}

/// Renders line `y` of the OBJ layer: colours into `colors`, the
/// matching priority attribute into `priorities`.
pub fn render_line(
    io: &IoRegisters,
    video: &VideoMemory,
    y: usize,
    colors: &mut [u16; SCREEN_WIDTH],
    priorities: &mut [u8; SCREEN_WIDTH],
) {
    colors.fill(TRANSPARENT);
    let dispcnt = io.read16(reg::DISPCNT);
    let one_dimensional = dispcnt & (1 << 6) != 0;
    let bitmap_mode = (dispcnt & 0x7) >= 3;
    let y = y as i32;

    for index in 0..OBJ_COUNT {
        let Some(obj) = Object::read(video, index) else {
            continue;
        };
        let (box_w, box_h) = obj.box_size();
        if obj.hidden || y < obj.y || y >= obj.y + box_h as i32 {
            continue;
        }
        let iy = (y - obj.y) as usize;

        let matrix = obj.affine.map(|i| affine_params(video, i));
        let (half_w, half_h) = (box_w as i32 / 2, box_h as i32 / 2);

        for ix in 0..box_w {
            let sx = obj.x + ix as i32;
            if !(0..SCREEN_WIDTH as i32).contains(&sx) {
                continue;
            }
            let sx = sx as usize;
            if colors[sx] != TRANSPARENT {
                continue; // an earlier object already owns this pixel
            }

            let (tx, ty) = if let Some([pa, pb, pc, pd]) = matrix {
                let (dx, dy) = (ix as i32 - half_w, iy as i32 - half_h);
                let tx = ((pa * dx + pb * dy) >> 8) + obj.width as i32 / 2;
                let ty = ((pc * dx + pd * dy) >> 8) + obj.height as i32 / 2;
                if tx < 0 || ty < 0 || tx >= obj.width as i32 || ty >= obj.height as i32 {
                    continue;
                }
                (tx as usize, ty as usize)
            } else {
                let tx = if obj.hflip { obj.width - 1 - ix } else { ix };
                let ty = if obj.vflip { obj.height - 1 - iy } else { iy };
                (tx, ty)
            };

            let color = texel(video, &obj, one_dimensional, bitmap_mode, tx, ty);
            if color != TRANSPARENT {
                colors[sx] = color;
                priorities[sx] = obj.priority;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_oam(video: &mut VideoMemory, index: usize, a0: u16, a1: u16, a2: u16) {
        let b = index * 8;
        video.oam[b..b + 2].copy_from_slice(&a0.to_le_bytes());
        video.oam[b + 2..b + 4].copy_from_slice(&a1.to_le_bytes());
        video.oam[b + 4..b + 6].copy_from_slice(&a2.to_le_bytes());
    }

    /// OBJ palette 1 colour 1 = red, 256-colour index 7 = blue.
    fn setup() -> (IoRegisters, VideoMemory) {
        let mut video = VideoMemory::new();
        let p = OBJ_PALETTE_BASE + 17 * 2;
        video.palette[p..p + 2].copy_from_slice(&0x001Fu16.to_le_bytes());
        let p = OBJ_PALETTE_BASE + 7 * 2;
        video.palette[p..p + 2].copy_from_slice(&0x7C00u16.to_le_bytes());
        (IoRegisters::new(), video)
    }

    fn render(
        io: &IoRegisters,
        video: &VideoMemory,
        y: usize,
    ) -> ([u16; SCREEN_WIDTH], [u8; SCREEN_WIDTH]) {
        let mut colors = [0; SCREEN_WIDTH];
        let mut prios = [0; SCREEN_WIDTH];
        render_line(io, video, y, &mut colors, &mut prios);
        (colors, prios)
    }

    #[test]
    fn simple_8x8_sprite_with_flip_and_priority() {
        let (io, mut video) = setup();
        // Tile 4, 4bpp: pixel (1, 2) = colour 1.
        video.vram[OBJ_VRAM_BASE + 4 * 32 + 2 * 4] = 0x10;
        // OBJ 0 at (10, 20), tile 4, palette 1, priority 2.
        write_oam(&mut video, 0, 20, 10, 4 | (2 << 10) | (1 << 12));
        let (colors, prios) = render(&io, &video, 22);
        assert_eq!(colors[11], 0x001F);
        assert_eq!(prios[11], 2);
        assert_eq!(colors[10], TRANSPARENT);
        assert_eq!(render(&io, &video, 21).0[11], TRANSPARENT);

        // Horizontal + vertical flip moves it to (6, 5) within the sprite.
        write_oam(
            &mut video,
            0,
            20,
            0x0A | (1 << 12) | (1 << 13),
            4 | (1 << 12),
        );
        let (colors, _) = render(&io, &video, 25);
        assert_eq!(colors[16], 0x001F);
    }

    #[test]
    fn disabled_and_window_objects_draw_nothing() {
        let (io, mut video) = setup();
        video.vram[OBJ_VRAM_BASE + 32] = 0x11;
        write_oam(&mut video, 0, 1 << 9, 0, 1 | (1 << 12)); // disabled
        assert_eq!(render(&io, &video, 0).0[0], TRANSPARENT);
        write_oam(&mut video, 0, 2 << 10, 0, 1 | (1 << 12)); // obj window
        assert_eq!(render(&io, &video, 0).0[0], TRANSPARENT);
        write_oam(&mut video, 0, 0, 0, 1 | (1 << 12));
        assert_eq!(render(&io, &video, 0).0[0], 0x001F);
    }

    #[test]
    fn oam_order_beats_priority_between_sprites() {
        let (io, mut video) = setup();
        video.vram[OBJ_VRAM_BASE + 32] = 0x11; // tile 1: pixels 0,1 colour 1
        video.vram[OBJ_VRAM_BASE + 64] = 0x77; // tile 2: 8bpp? no - 4bpp colour 7
        write_oam(&mut video, 0, 0, 0, 1 | (3 << 10) | (1 << 12)); // prio 3, palette 1
        write_oam(&mut video, 1, 0, 0, 2); // prio 0, palette 0
        let (colors, prios) = render(&io, &video, 0);
        assert_eq!(
            colors[0], 0x001F,
            "OBJ 0 wins although its priority is worse"
        );
        assert_eq!(prios[0], 3);
    }

    #[test]
    fn negative_x_and_wrapped_y() {
        let (io, mut video) = setup();
        // 16x16 sprite (square, size 1), tile 0 with pixel (15, 15) set:
        // 2D mapping -> tile row 1 is tile 32, column 1 -> tile 33, pixel (7,7).
        video.vram[OBJ_VRAM_BASE + 33 * 32 + 7 * 4 + 3] = 0x10;
        // x = -8 (0x1F8), y = 250 -> wraps to -6; pixel lands at (7, 9).
        write_oam(&mut video, 0, 250, 0x1F8 | (1 << 14), 1 << 12);
        let (colors, _) = render(&io, &video, 9);
        assert_eq!(colors[7], 0x001F);
    }

    #[test]
    fn one_dimensional_mapping_and_256_colours() {
        let (mut io, mut video) = setup();
        io.write16(reg::DISPCNT, 1 << 6); // 1D mapping
        // 16x8 (horizontal, size 0), 8bpp, tile 10: second tile is tile 12 in
        // 1D mode (two 32-byte units per 8bpp tile). Pixel (9, 3) = index 7.
        video.vram[OBJ_VRAM_BASE + 12 * 32 + 3 * 8 + 1] = 7;
        write_oam(&mut video, 0, (1 << 13) | (1 << 14), 0, 10);
        let (colors, _) = render(&io, &video, 3);
        assert_eq!(colors[9], 0x7C00);
        assert_eq!(colors[8], TRANSPARENT);
    }

    #[test]
    fn affine_sprite_identity_and_double_size() {
        let (io, mut video) = setup();
        // 8x8 sprite, tile 1, pixel (0, 0) = colour 1.
        video.vram[OBJ_VRAM_BASE + 32] = 0x01;
        // Affine group 0 = identity.
        for (i, v) in [0x100u16, 0, 0, 0x100].iter().enumerate() {
            let o = 6 + i * 8;
            video.oam[o..o + 2].copy_from_slice(&v.to_le_bytes());
        }
        write_oam(&mut video, 0, 1 << 8, 0, 1 | (1 << 12));
        assert_eq!(render(&io, &video, 0).0[0], 0x001F);
        // Double size: the 16x16 box is centred, so texel (0,0) is at (4,4).
        write_oam(&mut video, 0, (1 << 8) | (1 << 9), 0, 1 | (1 << 12));
        let (colors, _) = render(&io, &video, 4);
        assert_eq!(colors[4], 0x001F);
        assert_eq!(colors[0], TRANSPARENT);
        assert_eq!(render(&io, &video, 0).0[0], TRANSPARENT);
    }
}
