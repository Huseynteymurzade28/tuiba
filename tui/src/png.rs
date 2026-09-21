//! A minimal PNG writer for screenshots.
//!
//! Emits 8-bit RGB with the image data in *stored* (uncompressed) deflate
//! blocks. That is a valid zlib stream every decoder accepts, and it
//! keeps the frontend free of compression dependencies; a 240×160 frame
//! is ~115 KiB, which is fine for debugging output.

use std::io::{self, Write};

/// Writes `rgb` (row-major, 3 bytes per pixel) as a PNG.
///
/// # Panics
///
/// If `rgb.len() != width * height * 3`.
pub fn write_rgb<W: Write>(mut out: W, width: u32, height: u32, rgb: &[u8]) -> io::Result<()> {
    let row_len = width as usize * 3;
    assert_eq!(rgb.len(), row_len * height as usize, "pixel buffer size");

    // Each scanline is prefixed with filter type 0 (None).
    let mut raw = Vec::with_capacity((row_len + 1) * height as usize);
    for row in rgb.chunks_exact(row_len) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, deflate, no filter, no interlace

    out.write_all(b"\x89PNG\r\n\x1a\n")?;
    write_chunk(&mut out, *b"IHDR", &ihdr)?;
    write_chunk(&mut out, *b"IDAT", &zlib_stored(&raw))?;
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

/// Wraps `data` in a zlib stream of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 0xFFFF;
    let mut z = Vec::with_capacity(data.len() + data.len() / MAX_BLOCK * 5 + 11);
    z.extend_from_slice(&[0x78, 0x01]); // CM=8 (deflate), 32K window, no preset dict

    let mut blocks = data.chunks(MAX_BLOCK).peekable();
    // An empty image still needs one (final, empty) block.
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let last = u8::from(blocks.peek().is_none());
        let len = block.len() as u16;
        z.push(last);
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
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

    #[test]
    fn stored_stream_round_trips() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7) as u8).collect();
        let z = zlib_stored(&data);
        // Decode the stored blocks by hand.
        assert_eq!(&z[..2], &[0x78, 0x01]);
        let mut pos = 2;
        let mut decoded = Vec::new();
        loop {
            let last = z[pos];
            let len = u16::from_le_bytes([z[pos + 1], z[pos + 2]]) as usize;
            let nlen = u16::from_le_bytes([z[pos + 3], z[pos + 4]]);
            assert_eq!(!(len as u16), nlen);
            pos += 5;
            decoded.extend_from_slice(&z[pos..pos + len]);
            pos += len;
            if last == 1 {
                break;
            }
        }
        assert_eq!(decoded, data);
        assert_eq!(&z[pos..], &adler32(&data).to_be_bytes());
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
