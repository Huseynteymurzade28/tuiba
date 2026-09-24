//! A minimal PNG writer, for screenshots and iTerm2 inline images.
//!
//! Emits 8-bit RGB. Each scanline is filtered (`Up` when it repeats the
//! one above, `Sub` otherwise), which turns an upscaled frame into long
//! runs of zeros, and the runs are compressed with a single fixed-Huffman
//! deflate block that only ever matches the byte just before. That is far
//! from what zlib achieves on photographs, but a 4× GBA frame shrinks from
//! 1.8 MiB to a few dozen KiB, and it keeps the frontend free of
//! compression dependencies.

use std::io::{self, Write};

/// Writes `rgb` (row-major, 3 bytes per pixel) as a PNG.
///
/// # Panics
///
/// If `rgb.len() != width * height * 3`.
pub fn write_rgb<W: Write>(mut out: W, width: u32, height: u32, rgb: &[u8]) -> io::Result<()> {
    let row_len = width as usize * 3;
    assert_eq!(rgb.len(), row_len * height as usize, "pixel buffer size");

    let mut raw = Vec::with_capacity((row_len + 1) * height as usize);
    let mut above: Option<&[u8]> = None;
    for row in rgb.chunks_exact(row_len) {
        if above == Some(row) {
            // Up: the difference from the row above, all zeros.
            raw.push(2);
            raw.resize(raw.len() + row_len, 0);
        } else {
            // Sub: the difference from the pixel to the left.
            raw.push(1);
            raw.extend_from_slice(&row[..row_len.min(3)]);
            raw.extend(
                row.iter()
                    .skip(3)
                    .zip(row)
                    .map(|(&b, &left)| b.wrapping_sub(left)),
            );
        }
        above = Some(row);
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, deflate, adaptive filters, no interlace

    out.write_all(b"\x89PNG\r\n\x1a\n")?;
    write_chunk(&mut out, *b"IHDR", &ihdr)?;
    write_chunk(&mut out, *b"IDAT", &zlib_runs(&raw))?;
    write_chunk(&mut out, *b"IEND", &[])?;
    out.flush()
}

fn write_chunk<W: Write>(out: &mut W, kind: [u8; 4], data: &[u8]) -> io::Result<()> {
    out.write_all(&(data.len() as u32).to_be_bytes())?;
    out.write_all(&kind)?;
    out.write_all(data)?;
    let mut crc = crc32(&kind, 0xFFFF_FFFF);
    crc = crc32(data, crc);
    out.write_all(&(!crc).to_be_bytes())
}

/// Deflate length codes 257..=285: the shortest length of each.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra bits after each length code.
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Longest match deflate can express.
const MAX_MATCH: usize = 258;

/// Bits packed least significant first, as deflate wants them.
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    count: u32,
}

impl BitWriter {
    fn bits(&mut self, value: u32, count: u32) {
        self.acc |= value << self.count;
        self.count += count;
        while self.count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code, which deflate sends most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        self.bits(code.reverse_bits() >> (32 - len), len);
    }

    /// A literal/length symbol of the fixed Huffman code.
    fn symbol(&mut self, symbol: u16) {
        let s = u32::from(symbol);
        match symbol {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + s - 144, 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xC0 + s - 280, 8),
        }
    }

    /// A match of `len` bytes at distance 1.
    fn repeat(&mut self, len: usize) {
        let len = len as u16;
        let i = LENGTH_BASE
            .iter()
            .rposition(|&base| base <= len)
            .unwrap_or(0);
        self.symbol(257 + i as u16);
        self.bits(u32::from(len - LENGTH_BASE[i]), u32::from(LENGTH_EXTRA[i]));
        // Distance code 0 (distance 1), no extra bits.
        self.code(0, 5);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// Wraps `data` in a zlib stream of one fixed-Huffman block, with runs
/// of a repeated byte as distance-1 matches.
fn zlib_runs(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter {
        out: Vec::with_capacity(data.len() / 8 + 16),
        acc: 0,
        count: 0,
    };
    w.out.extend_from_slice(&[0x78, 0x01]); // CM=8 (deflate), 32K window, no preset dict
    w.bits(1, 1); // BFINAL
    w.bits(1, 2); // BTYPE = fixed Huffman
    let mut i = 0;
    while i < data.len() {
        if i > 0 {
            let previous = data[i - 1];
            let run = data[i..]
                .iter()
                .take(MAX_MATCH)
                .take_while(|&&b| b == previous)
                .count();
            if run >= 3 {
                w.repeat(run);
                i += run;
                continue;
            }
        }
        w.symbol(u16::from(data[i]));
        i += 1;
    }
    w.symbol(256); // end of block
    let mut z = w.finish();
    z.extend_from_slice(&adler32(data).to_be_bytes());
    z
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest chunk for which the sums cannot overflow u32.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// Continues a CRC-32 (IEEE) over `data`; start with `0xFFFF_FFFF` and
/// invert the result.
fn crc32(data: &[u8], mut crc: u32) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(!crc32(b"123456789", 0xFFFF_FFFF), 0xCBF4_3926);
        // The IEND chunk's CRC is a well-known constant.
        assert_eq!(!crc32(b"IEND", 0xFFFF_FFFF), 0xAE42_6082);
    }

    #[test]
    fn adler32_matches_reference() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    /// Inflates a zlib stream of fixed-Huffman blocks with distance-1
    /// matches: all `zlib_runs` produces.
    fn inflate(z: &[u8]) -> Vec<u8> {
        assert_eq!(&z[..2], &[0x78, 0x01]);
        let mut bit = 16; // position in bits
        let mut read = |n: u32| {
            let mut v = 0;
            for k in 0..n {
                v |= u32::from(z[bit / 8] >> (bit % 8) & 1) << k;
                bit += 1;
            }
            v
        };
        assert_eq!(read(1), 1, "final block");
        assert_eq!(read(2), 1, "fixed Huffman");
        let mut out: Vec<u8> = Vec::new();
        loop {
            // Huffman codes arrive most significant bit first.
            let mut code = 0;
            let mut len = 0;
            let symbol = loop {
                code = code << 1 | read(1);
                len += 1;
                match (len, code) {
                    (7, 0..=23) => break code + 256,
                    (8, 0x30..=0xBF) => break code - 0x30,
                    (8, 0xC0..=0xC7) => break code - 0xC0 + 280,
                    (9, 0x190..=0x1FF) => break code - 0x190 + 144,
                    (9, _) => panic!("bad code"),
                    _ => {}
                }
            };
            match symbol {
                0..=255 => out.push(symbol as u8),
                256 => break,
                _ => {
                    let i = (symbol - 257) as usize;
                    let len = LENGTH_BASE[i] as usize + read(u32::from(LENGTH_EXTRA[i])) as usize;
                    let mut dist = 0;
                    for _ in 0..5 {
                        dist = dist << 1 | read(1);
                    }
                    assert_eq!(dist, 0, "distance 1");
                    for _ in 0..len {
                        out.push(*out.last().unwrap());
                    }
                }
            }
        }
        let end = bit.div_ceil(8);
        assert_eq!(&z[end..], &adler32(&out).to_be_bytes());
        out
    }

    #[test]
    fn run_stream_round_trips() {
        let mut data: Vec<u8> = (0..20_000u32).map(|i| (i * 7) as u8).collect();
        data.extend(std::iter::repeat_n(0, 1000));
        data.extend([5, 5, 5, 9, 9, 200, 200, 200, 200, 255, 144, 143]);
        data.extend(std::iter::repeat_n(0xAB, 259));
        for len in [0, 1, 2, 3, 4, 10, 11, 12, 257, 258, 259] {
            data.push(1);
            data.extend(std::iter::repeat_n(2, len));
        }
        assert_eq!(inflate(&zlib_runs(&data)), data);
        assert_eq!(inflate(&zlib_runs(&[])), Vec::<u8>::new());
    }

    #[test]
    fn upscaled_frames_compress_well() {
        // 960 × 640: a 240 × 160 pattern of 8-pixel stripes at 4×.
        let (w, h) = (960, 640);
        let rgb: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [(x / 32 * 3) as u8, (y / 4) as u8, 7]))
            .collect();
        let mut png = Vec::new();
        write_rgb(&mut png, w as u32, h as u32, &rgb).unwrap();
        assert!(png.len() < rgb.len() / 50, "{} bytes", png.len());
    }

    #[test]
    fn writes_well_formed_file() {
        let mut buf = Vec::new();
        write_rgb(&mut buf, 2, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
        assert_eq!(&buf[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&buf[8..16], b"\x00\x00\x00\x0dIHDR");
        assert_eq!(
            &buf[buf.len() - 12..],
            b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"
        );
    }
}
