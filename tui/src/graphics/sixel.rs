//! Sixel output.
//!
//! Sixel images are paletted: the frame is reduced to at most 256
//! colours first. Most GBA frames already fit, since the hardware draws
//! from 512 palette entries and most scenes use far fewer; frames that do
//! not (heavy blending, gradients) lose low colour bits until they fit.
//! The image is then cut into bands six pixels tall, and each colour of a
//! band is sent as one run-length-encoded line of six-bit columns.
//!
//! Reference: <https://vt100.net/docs/vt3xx-gp/chapter14.html>

use std::io::{self, Write};

use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use super::Placement;

/// Colour registers we use; the common minimum among sixel terminals.
const MAX_COLORS: usize = 256;
/// Bits kept per channel (red, green, blue), tried in order until the
/// frame fits the palette. The last always fits: 3 + 3 + 2 bits.
const DEPTHS: [(u32, u32, u32); 4] = [(5, 5, 5), (4, 4, 4), (3, 3, 3), (3, 3, 2)];
/// Marks an unused lookup entry.
const NONE: u16 = u16::MAX;

/// Encoder scratch, kept between frames.
#[derive(Debug)]
pub(super) struct Sixel {
    /// The frame as palette indices, at native size.
    indices: Vec<u8>,
    /// The same, scaled to the output size.
    scaled: Vec<u8>,
    palette: Vec<[u8; 3]>,
    /// Reduced colour → palette index.
    lookup: Vec<u16>,
    /// Lookup entries in use, to reset them cheaply.
    keys: Vec<u16>,
    /// One line of six-bit columns per colour present in the band.
    bits: Vec<u8>,
    /// Palette index → line in `bits` for the current band.
    line_of: Vec<u16>,
    /// Palette indices present in the current band, in order seen.
    band_colors: Vec<u8>,
}

impl Default for Sixel {
    fn default() -> Self {
        Self {
            indices: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT],
            scaled: Vec::new(),
            palette: Vec::with_capacity(MAX_COLORS),
            lookup: vec![NONE; 1 << 15],
            keys: Vec::with_capacity(MAX_COLORS),
            bits: Vec::new(),
            line_of: vec![NONE; MAX_COLORS],
            band_colors: Vec::with_capacity(MAX_COLORS),
        }
    }
}

impl Sixel {
    /// Appends the sixel image of `fb`, sized as `placement` says.
    pub(super) fn encode(
        &mut self,
        fb: &Framebuffer,
        placement: Placement,
        out: &mut Vec<u8>,
    ) -> io::Result<()> {
        self.quantize(fb);
        self.scale(placement.width, placement.height);
        self.write(placement.width, placement.height, out)
    }

    /// Fills `palette` and `indices` from `fb`, with as many colour bits
    /// as fit in [`MAX_COLORS`].
    fn quantize(&mut self, fb: &Framebuffer) {
        for (rb, gb, bb) in DEPTHS {
            for key in self.keys.drain(..) {
                self.lookup[usize::from(key)] = NONE;
            }
            self.palette.clear();
            if self.try_quantize(fb, rb, gb, bb) {
                return;
            }
        }
        unreachable!("3-3-2 bits always fit in 256 colours");
    }

    fn try_quantize(&mut self, fb: &Framebuffer, rb: u32, gb: u32, bb: u32) -> bool {
        for y in 0..SCREEN_HEIGHT {
            let row = &mut self.indices[y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH];
            for (index, &p) in row.iter_mut().zip(fb.row(y)) {
                let [r, g, b, _] = p.to_be_bytes();
                let key = (u32::from(r) >> (8 - rb) << (gb + bb))
                    | (u32::from(g) >> (8 - gb) << bb)
                    | (u32::from(b) >> (8 - bb));
                // At most 15 bits by construction.
                let key = key as u16;
                let mut slot = self.lookup[usize::from(key)];
                if slot == NONE {
                    if self.palette.len() == MAX_COLORS {
                        return false;
                    }
                    slot = self.palette.len() as u16;
                    self.lookup[usize::from(key)] = slot;
                    self.keys.push(key);
                    // The first colour seen stands for its whole bucket.
                    self.palette.push([r, g, b]);
                }
                *index = slot as u8;
            }
        }
        true
    }

    /// Nearest-neighbour scales `indices` to `width` × `height` into
    /// `scaled`.
    fn scale(&mut self, width: usize, height: usize) {
        let columns: Vec<usize> = (0..width).map(|x| x * SCREEN_WIDTH / width).collect();
        self.scaled.clear();
        self.scaled.reserve(width * height);
        let mut last_source = usize::MAX;
        for y in 0..height {
            let source = y * SCREEN_HEIGHT / height;
            if source == last_source {
                // Repeated source row: copy the scaled one above.
                let start = self.scaled.len() - width;
                self.scaled.extend_from_within(start..start + width);
                continue;
            }
            last_source = source;
            let row = &self.indices[source * SCREEN_WIDTH..(source + 1) * SCREEN_WIDTH];
            self.scaled.extend(columns.iter().map(|&x| row[x]));
        }
    }

    /// Appends the DCS sequence for `scaled`.
    fn write(&mut self, width: usize, height: usize, out: &mut Vec<u8>) -> io::Result<()> {
        // P2 = 1: bits left at zero keep what is underneath, so the band
        // padding below the last row paints nothing. Raster attributes:
        // square pixels, exact size.
        write!(out, "\x1bP0;1;0q\"1;1;{width};{height}")?;
        for (n, &[r, g, b]) in self.palette.iter().enumerate() {
            write!(out, "#{n};2;{};{};{}", percent(r), percent(g), percent(b))?;
        }
        if self.bits.len() < MAX_COLORS * width {
            self.bits.resize(MAX_COLORS * width, 0);
        }
        for top in (0..height).step_by(6) {
            if top > 0 {
                out.push(b'-');
            }
            for dy in 0..6.min(height - top) {
                let row = &self.scaled[(top + dy) * width..(top + dy + 1) * width];
                for (x, &color) in row.iter().enumerate() {
                    let mut line = self.line_of[usize::from(color)];
                    if line == NONE {
                        line = self.band_colors.len() as u16;
                        self.line_of[usize::from(color)] = line;
                        self.band_colors.push(color);
                    }
                    self.bits[usize::from(line) * width + x] |= 1 << dy;
                }
            }
            for (line, &color) in self.band_colors.iter().enumerate() {
                if line > 0 {
                    // Back to the start of the band for the next colour.
                    out.push(b'$');
                }
                write!(out, "#{color}")?;
                let bits = &mut self.bits[line * width..(line + 1) * width];
                run_length(bits, out);
                bits.fill(0);
                self.line_of[usize::from(color)] = NONE;
            }
            self.band_colors.clear();
        }
        out.extend_from_slice(b"\x1b\\");
        Ok(())
    }
}

/// A colour channel as the 0–100 scale sixel palettes use.
fn percent(c: u8) -> u32 {
    (u32::from(c) * 100 + 127) / 255
}

/// Appends one line of six-bit columns, with runs of four or more as
/// `!count char` and trailing empty columns dropped.
fn run_length(bits: &[u8], out: &mut Vec<u8>) {
    let end = bits.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let mut rest = &bits[..end];
    while let Some(&first) = rest.first() {
        let run = rest.iter().take_while(|&&b| b == first).count();
        let char = first + 63;
        if run >= 4 {
            // Writing to a Vec cannot fail.
            let _ = write!(out, "!{run}");
            out.push(char);
        } else {
            out.resize(out.len() + run, char);
        }
        rest = &rest[run..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal sixel decoder: palette and pixel indices (`None` where
    /// nothing was painted).
    fn decode(data: &[u8]) -> (usize, usize, Vec<[u8; 3]>, Vec<Option<u8>>) {
        let body = data
            .strip_prefix(b"\x1bP0;1;0q\"")
            .and_then(|d| d.strip_suffix(b"\x1b\\"))
            .expect("DCS framing");
        let mut pos = 0;
        let number = |pos: &mut usize| {
            let start = *pos;
            while body[*pos].is_ascii_digit() {
                *pos += 1;
            }
            std::str::from_utf8(&body[start..*pos])
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        let mut raster = [0; 4];
        for (n, value) in raster.iter_mut().enumerate() {
            if n > 0 {
                assert_eq!(body[pos], b';');
                pos += 1;
            }
            *value = number(&mut pos);
        }
        let [_, _, width, height] = raster;
        let mut palette = vec![[0; 3]; MAX_COLORS];
        let mut pixels = vec![None; width * height];
        let (mut x, mut top, mut color) = (0, 0, 0);
        while pos < body.len() {
            match body[pos] {
                b'#' => {
                    pos += 1;
                    color = number(&mut pos);
                    if body.get(pos) == Some(&b';') {
                        pos += 3; // ";2;"
                        let r = number(&mut pos);
                        pos += 1;
                        let g = number(&mut pos);
                        pos += 1;
                        let b = number(&mut pos);
                        palette[color] = [r, g, b].map(|c| (c * 255 / 100) as u8);
                    }
                }
                b'$' => {
                    x = 0;
                    pos += 1;
                }
                b'-' => {
                    x = 0;
                    top += 6;
                    pos += 1;
                }
                b'!' | 63..=126 => {
                    let count = if body[pos] == b'!' {
                        pos += 1;
                        number(&mut pos)
                    } else {
                        1
                    };
                    let six = body[pos] - 63;
                    pos += 1;
                    for _ in 0..count {
                        for dy in 0..6 {
                            if six & (1 << dy) != 0 {
                                assert!(top + dy < height, "painted below the image");
                                pixels[(top + dy) * width + x] = Some(color as u8);
                            }
                        }
                        x += 1;
                    }
                }
                other => panic!("unexpected byte {other:#x} at {pos}"),
            }
        }
        (width, height, palette, pixels)
    }

    fn placement(width: usize, height: usize) -> Placement {
        Placement {
            x: 0,
            y: 0,
            cols: 1,
            rows: 1,
            factor: width / SCREEN_WIDTH,
            shrink: width < SCREEN_WIDTH,
            width,
            height,
        }
    }

    fn rgb(p: u32) -> [u8; 3] {
        let [r, g, b, _] = p.to_be_bytes();
        [r, g, b]
    }

    /// A frame with a few hundred distinct GBA colours.
    fn busy_frame() -> Framebuffer {
        let mut fb = Framebuffer::new();
        for y in 0..SCREEN_HEIGHT {
            for (x, p) in fb.row_mut(y).iter_mut().enumerate() {
                let c5 = |v: usize| {
                    let v = (v % 32) as u32;
                    (v << 3) | (v >> 2)
                };
                *p = (c5(x / 8) << 24) | (c5(y / 8) << 16) | (c5(x / 16 + y / 16) << 8) | 0xFF;
            }
        }
        fb
    }

    #[test]
    fn round_trips_a_frame_at_twice_the_size() {
        let mut fb = Framebuffer::new();
        fb.row_mut(0)[0] = 0xFF00_00FF;
        fb.row_mut(5)[7] = 0x00FF_00FF;
        fb.row_mut(159)[239] = 0x0000_FFFF;
        let mut sixel = Sixel::default();
        let mut out = Vec::new();
        sixel.encode(&fb, placement(480, 320), &mut out).unwrap();
        let (w, h, palette, pixels) = decode(&out);
        assert_eq!((w, h), (480, 320));
        for y in 0..h {
            for x in 0..w {
                let want = rgb(fb.row(y / 2)[x / 2]);
                let got = palette[usize::from(pixels[y * w + x].expect("painted"))];
                assert_eq!(got, want, "pixel {x},{y}");
            }
        }
        // A black screen with three dots compresses to next to nothing.
        assert!(out.len() < 4_000, "{} bytes", out.len());
    }

    #[test]
    fn heights_that_are_not_a_multiple_of_six_paint_nothing_below() {
        let fb = busy_frame();
        let mut out = Vec::new();
        // 160 × 107: a shrunk image, 107 = 17 bands + 5 rows.
        Sixel::default()
            .encode(&fb, placement(160, 107), &mut out)
            .unwrap();
        let (w, h, _, pixels) = decode(&out);
        assert_eq!((w, h), (160, 107));
        assert!(pixels.iter().all(Option::is_some));
    }

    #[test]
    fn frames_with_too_many_colours_lose_bits_until_they_fit() {
        let fb = busy_frame();
        let mut sixel = Sixel::default();
        let mut out = Vec::new();
        sixel.encode(&fb, placement(240, 160), &mut out).unwrap();
        assert!(sixel.palette.len() <= MAX_COLORS);
        let (_, _, palette, pixels) = decode(&out);
        // Every pixel lands within the dropped bits of its true colour.
        for y in 0..SCREEN_HEIGHT {
            for x in 0..SCREEN_WIDTH {
                let want = rgb(fb.row(y)[x]);
                let got = palette[usize::from(pixels[y * SCREEN_WIDTH + x].unwrap())];
                for (w, g) in want.iter().zip(got) {
                    assert!(w.abs_diff(g) < 64, "pixel {x},{y}: {want:?} vs {got:?}");
                }
            }
        }

        // A plain frame afterwards gets its exact colours back.
        let fb = Framebuffer::new();
        sixel.encode(&fb, placement(240, 160), &mut out).unwrap();
        assert_eq!(sixel.palette, vec![[0, 0, 0]]);
    }

    #[test]
    fn runs_are_compressed() {
        let mut out = Vec::new();
        run_length(&[1, 1, 1, 1, 1, 2, 2, 0, 0, 0, 0], &mut out);
        assert_eq!(out, b"!5@AA");
    }
}
