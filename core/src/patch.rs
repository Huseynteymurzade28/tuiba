//! ROM patches: IPS, UPS and BPS.
//!
//! Translations and ROM hacks are distributed as patches against the
//! original image rather than as ROMs. [`apply`] recognises the format
//! from the patch's first bytes and returns the patched image, which is
//! then loaded like any other ROM — its fingerprint differs from the
//! original's, so save states of the two never mix.
//!
//! UPS and BPS carry CRC-32s of the image they expect, the image they
//! produce and themselves; all three are checked, so a patch for a
//! different revision of a game is refused instead of producing
//! something that crashes later. IPS has no checksums and applies to
//! whatever it is given.

use crate::error::{GbaError, Result};

/// The patch formats [`apply`] understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// International Patching System: offsets and bytes, no checks.
    Ips,
    /// Universal Patching System: XOR runs, with CRC-32s.
    Ups,
    /// Beat Patching System: copies from source, target or patch, with
    /// CRC-32s.
    Bps,
}

impl Format {
    /// Recognises a patch by its magic bytes.
    #[must_use]
    pub fn detect(patch: &[u8]) -> Option<Self> {
        if patch.starts_with(b"PATCH") {
            Some(Self::Ips)
        } else if patch.starts_with(b"UPS1") {
            Some(Self::Ups)
        } else if patch.starts_with(b"BPS1") {
            Some(Self::Bps)
        } else {
            None
        }
    }

    /// The file extension patches of this format use.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ips => "ips",
            Self::Ups => "ups",
            Self::Bps => "bps",
        }
    }
}

/// Ways a patch can fail to apply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PatchError {
    /// The bytes do not start like any known patch format.
    #[error("not an IPS, UPS or BPS patch")]
    UnknownFormat,
    /// The patch ends in the middle of a record.
    #[error("patch is truncated")]
    Truncated,
    /// The patch's own checksum does not match: it was damaged in
    /// transit.
    #[error("patch is damaged (checksum mismatch)")]
    Damaged,
    /// The patch was made for a different ROM (or a different revision
    /// of this one).
    #[error("patch is for a different ROM")]
    WrongSource,
    /// Applying the patch did not produce the image it promised.
    #[error("patched ROM does not match the patch's checksum")]
    BadResult,
    /// A copy in the patch reaches outside the image it copies from.
    #[error("patch reads outside the ROM")]
    OutOfRange,
}

impl From<PatchError> for GbaError {
    fn from(err: PatchError) -> Self {
        Self::Patch(err)
    }
}

/// Applies `patch` to `rom` and returns the patched image.
///
/// # Errors
///
/// [`GbaError::Patch`] with the reason when the patch is not one of the
/// supported formats, is damaged, or does not belong to `rom`.
pub fn apply(rom: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    let patched = match Format::detect(patch).ok_or(PatchError::UnknownFormat)? {
        Format::Ips => ips(rom, patch),
        Format::Ups => ups(rom, patch),
        Format::Bps => bps(rom, patch),
    }?;
    Ok(patched)
}

/// IPS: after `PATCH`, records of a 3-byte offset and a 2-byte length
/// followed by that many bytes — or, when the length is zero, a 2-byte
/// count and one byte to repeat. `EOF` ends the list, optionally
/// followed by a 3-byte size to truncate the image to.
fn ips(rom: &[u8], patch: &[u8]) -> std::result::Result<Vec<u8>, PatchError> {
    let mut out = rom.to_vec();
    let mut r = Reader::new(&patch[5..]);
    loop {
        let offset = r.bytes(3)?;
        if offset == b"EOF" {
            if let Ok(size) = r.bytes(3) {
                out.truncate(be(size));
            }
            return Ok(out);
        }
        let offset = be(offset);
        let len = be(r.bytes(2)?);
        if len == 0 {
            let count = be(r.bytes(2)?);
            let value = r.byte()?;
            write_at(&mut out, offset, &vec![value; count]);
        } else {
            write_at(&mut out, offset, r.bytes(len)?);
        }
    }
}

/// Copies `data` into `out` at `offset`, growing it (with zeros) when a
/// record reaches past the end: IPS patches can extend a ROM.
fn write_at(out: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if out.len() < end {
        out.resize(end, 0);
    }
    out[offset..end].copy_from_slice(data);
}

/// Big-endian number from up to four bytes.
fn be(bytes: &[u8]) -> usize {
    bytes.iter().fold(0, |n, &b| (n << 8) | usize::from(b))
}

/// UPS: sizes of source and target, then runs of "skip this many bytes,
/// XOR these in until a zero", then the three checksums.
fn ups(rom: &[u8], patch: &[u8]) -> std::result::Result<Vec<u8>, PatchError> {
    let body = checked_body(patch)?;
    let (source_crc, target_crc) = footer_crcs(patch);
    if crc32(rom) != source_crc {
        return Err(PatchError::WrongSource);
    }
    let mut r = Reader::new(&body[4..]);
    let source_size = r.number()?;
    let target_size = r.number()?;
    if source_size != rom.len() {
        return Err(PatchError::WrongSource);
    }
    let mut out = rom.to_vec();
    out.resize(target_size, 0);
    let mut pos = 0;
    while !r.is_empty() {
        pos += r.number()?;
        loop {
            let x = r.byte()?;
            if x == 0 {
                break;
            }
            if let Some(b) = out.get_mut(pos) {
                *b ^= x;
            }
            pos += 1;
        }
        // The terminating zero stands for one unchanged byte.
        pos += 1;
    }
    if crc32(&out) != target_crc {
        return Err(PatchError::BadResult);
    }
    Ok(out)
}

/// BPS: sizes, metadata, then actions that build the target front to
/// back — copy the source at the same offset, copy literal bytes from
/// the patch, or copy from a moving cursor in the source or in the
/// target written so far — then the three checksums.
fn bps(rom: &[u8], patch: &[u8]) -> std::result::Result<Vec<u8>, PatchError> {
    let body = checked_body(patch)?;
    let (source_crc, target_crc) = footer_crcs(patch);
    if crc32(rom) != source_crc {
        return Err(PatchError::WrongSource);
    }
    let mut r = Reader::new(&body[4..]);
    let source_size = r.number()?;
    let target_size = r.number()?;
    if source_size != rom.len() {
        return Err(PatchError::WrongSource);
    }
    let metadata = r.number()?;
    r.bytes(metadata)?;

    let mut out = Vec::with_capacity(target_size);
    let mut source_cursor = 0usize;
    let mut target_cursor = 0usize;
    while !r.is_empty() {
        let action = r.number()?;
        let len = (action >> 2) + 1;
        match action & 3 {
            // SourceRead: the source's bytes at the output's own offset.
            0 => {
                let at = out.len();
                out.extend_from_slice(rom.get(at..at + len).ok_or(PatchError::OutOfRange)?);
            }
            // TargetRead: literal bytes from the patch.
            1 => out.extend_from_slice(r.bytes(len)?),
            // SourceCopy: from anywhere in the source.
            2 => {
                source_cursor = moved(source_cursor, r.number()?)?;
                let from = rom
                    .get(source_cursor..source_cursor + len)
                    .ok_or(PatchError::OutOfRange)?;
                out.extend_from_slice(from);
                source_cursor += len;
            }
            // TargetCopy: from what has been written, byte by byte, as
            // the copy may overlap its own output (a run-length repeat).
            _ => {
                target_cursor = moved(target_cursor, r.number()?)?;
                for _ in 0..len {
                    let b = *out.get(target_cursor).ok_or(PatchError::OutOfRange)?;
                    out.push(b);
                    target_cursor += 1;
                }
            }
        }
        if out.len() > target_size {
            return Err(PatchError::BadResult);
        }
    }
    if out.len() != target_size || crc32(&out) != target_crc {
        return Err(PatchError::BadResult);
    }
    Ok(out)
}

/// A BPS cursor move: the low bit is the sign, the rest the distance.
fn moved(cursor: usize, delta: usize) -> std::result::Result<usize, PatchError> {
    let distance = delta >> 1;
    if delta & 1 == 0 {
        cursor.checked_add(distance)
    } else {
        cursor.checked_sub(distance)
    }
    .ok_or(PatchError::OutOfRange)
}

/// Length of the UPS/BPS checksum footer: source, target and patch
/// CRC-32s, little-endian.
const FOOTER_LEN: usize = 12;

/// The patch without its footer, once the footer's own checksum has
/// been verified.
fn checked_body(patch: &[u8]) -> std::result::Result<&[u8], PatchError> {
    if patch.len() < 4 + FOOTER_LEN {
        return Err(PatchError::Truncated);
    }
    let (covered, own) = patch.split_at(patch.len() - 4);
    if crc32(covered) != le32(own) {
        return Err(PatchError::Damaged);
    }
    Ok(&patch[..patch.len() - FOOTER_LEN])
}

/// The source and target CRC-32s from the footer.
fn footer_crcs(patch: &[u8]) -> (u32, u32) {
    let footer = &patch[patch.len() - FOOTER_LEN..];
    (le32(&footer[..4]), le32(&footer[4..8]))
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Cursor over a patch's bytes.
struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn byte(&mut self) -> std::result::Result<u8, PatchError> {
        let (&b, rest) = self.data.split_first().ok_or(PatchError::Truncated)?;
        self.data = rest;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> std::result::Result<&'a [u8], PatchError> {
        if n > self.data.len() {
            return Err(PatchError::Truncated);
        }
        let (taken, rest) = self.data.split_at(n);
        self.data = rest;
        Ok(taken)
    }

    /// The variable-length number UPS and BPS share: seven bits per
    /// byte, low first, the top bit ending it — with one added per
    /// continuation so that every number has exactly one encoding.
    fn number(&mut self) -> std::result::Result<usize, PatchError> {
        let mut n = 0usize;
        let mut shift = 1usize;
        loop {
            let b = self.byte()?;
            n = usize::from(b & 0x7F)
                .checked_mul(shift)
                .and_then(|v| n.checked_add(v))
                .ok_or(PatchError::Truncated)?;
            if b & 0x80 != 0 {
                return Ok(n);
            }
            shift = shift.checked_shl(7).ok_or(PatchError::Truncated)?;
            n = n.checked_add(shift).ok_or(PatchError::Truncated)?;
        }
    }
}

/// CRC-32 (IEEE 802.3, the zlib one), bit by bit: patches are applied
/// once per load, so a table buys nothing.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xEDB8_8320
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a number the way [`Reader::number`] reads it.
    fn number(mut n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let low = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(low | 0x80);
                return out;
            }
            out.push(low);
            n -= 1;
        }
    }

    /// Appends the three-checksum footer to a UPS or BPS body.
    fn with_footer(mut body: Vec<u8>, source: &[u8], target: &[u8]) -> Vec<u8> {
        body.extend_from_slice(&crc32(source).to_le_bytes());
        body.extend_from_slice(&crc32(target).to_le_bytes());
        let own = crc32(&body);
        body.extend_from_slice(&own.to_le_bytes());
        body
    }

    fn patch_error(result: Result<Vec<u8>>) -> PatchError {
        match result {
            Err(GbaError::Patch(err)) => err,
            other => panic!("expected a patch error, got {other:?}"),
        }
    }

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn numbers_round_trip() {
        for n in [0, 1, 127, 128, 129, 16_383, 16_384, 16_512, 1 << 25] {
            let bytes = number(n);
            assert_eq!(Reader::new(&bytes).number(), Ok(n), "{n}");
        }
    }

    #[test]
    fn formats_are_told_apart_by_magic() {
        assert_eq!(Format::detect(b"PATCHEOF"), Some(Format::Ips));
        assert_eq!(Format::detect(b"UPS1...."), Some(Format::Ups));
        assert_eq!(Format::detect(b"BPS1...."), Some(Format::Bps));
        assert_eq!(Format::detect(b"\x89PNG"), None);
        assert_eq!(
            patch_error(apply(b"rom", b"nonsense")),
            PatchError::UnknownFormat
        );
    }

    #[test]
    fn ips_writes_runs_and_repeats_and_can_grow() {
        let rom = b"hello world".to_vec();
        let mut patch = b"PATCH".to_vec();
        // "jello" at 0.
        patch.extend_from_slice(&[0, 0, 0, 0, 1, b'j']);
        // Three '!' starting at 11, past the end.
        patch.extend_from_slice(&[0, 0, 11, 0, 0, 0, 3, b'!']);
        patch.extend_from_slice(b"EOF");
        assert_eq!(apply(&rom, &patch).unwrap(), b"jello world!!!");
    }

    #[test]
    fn ips_can_truncate() {
        let mut patch = b"PATCH".to_vec();
        patch.extend_from_slice(b"EOF");
        patch.extend_from_slice(&[0, 0, 5]);
        assert_eq!(apply(b"hello world", &patch).unwrap(), b"hello");
    }

    #[test]
    fn ips_without_eof_is_truncated() {
        let patch = b"PATCH\x00\x00\x00\x00\x05ab".to_vec();
        assert_eq!(patch_error(apply(b"hello", &patch)), PatchError::Truncated);
    }

    /// "hello world" → "jello there": XOR runs at 0 and at 6..11.
    fn ups_patch(source: &[u8], target: &[u8]) -> Vec<u8> {
        let mut body = b"UPS1".to_vec();
        body.extend(number(source.len()));
        body.extend(number(target.len()));
        let mut pos = 0;
        let mut i = 0;
        while i < target.len() {
            let s = source.get(i).copied().unwrap_or(0);
            if s == target[i] {
                i += 1;
                continue;
            }
            body.extend(number(i - pos));
            while i < target.len() && source.get(i).copied().unwrap_or(0) != target[i] {
                body.push(source.get(i).copied().unwrap_or(0) ^ target[i]);
                i += 1;
            }
            body.push(0);
            i += 1;
            pos = i;
        }
        with_footer(body, source, target)
    }

    #[test]
    fn ups_applies_and_checks() {
        let (source, target) = (b"hello world", b"jello there, friend");
        let patch = ups_patch(source, target);
        assert_eq!(apply(source, &patch).unwrap(), target);
        assert_eq!(
            patch_error(apply(b"hello wOrld", &patch)),
            PatchError::WrongSource
        );
        let mut damaged = patch.clone();
        damaged[6] ^= 1;
        assert_eq!(patch_error(apply(source, &damaged)), PatchError::Damaged);
    }

    /// A BPS patch using every action: the source's "hello " in place,
    /// a literal "big ", "world" copied from the source, a literal "!",
    /// then a target copy that repeats the "!" as it writes it.
    fn bps_patch(source: &[u8], target: &[u8]) -> Vec<u8> {
        let action = |kind: usize, len: usize| number(((len - 1) << 2) | kind);
        let mut body = b"BPS1".to_vec();
        body.extend(number(source.len()));
        body.extend(number(target.len()));
        body.extend(number(3));
        body.extend_from_slice(b"meh");
        body.extend(action(0, 6)); // "hello "
        body.extend(action(1, 4)); // "big "
        body.extend_from_slice(b"big ");
        body.extend(action(2, 5)); // source "world" at 6
        body.extend(number(6 << 1));
        body.extend(action(1, 1)); // "!"
        body.push(b'!');
        body.extend(action(3, 3)); // "!!!" from the '!' at 15, overlapping
        body.extend(number(15 << 1));
        with_footer(body, source, target)
    }

    #[test]
    fn bps_applies_every_action() {
        let (source, target) = (b"hello world", b"hello big world!!!!");
        let patch = bps_patch(source, target);
        assert_eq!(apply(source, &patch).unwrap(), target);
    }

    #[test]
    fn bps_refuses_the_wrong_rom_and_a_damaged_patch() {
        let (source, target) = (b"hello world", b"hello big world!!!!");
        let patch = bps_patch(source, target);
        assert_eq!(
            patch_error(apply(b"HELLO WORLD", &patch)),
            PatchError::WrongSource
        );
        let mut damaged = patch.clone();
        let last = damaged.len() - 13;
        damaged[last] ^= 0x40;
        assert_eq!(patch_error(apply(source, &damaged)), PatchError::Damaged);
    }

    #[test]
    fn bps_copies_outside_the_source_are_refused_not_panicked_on() {
        let source = b"abc";
        let target = b"abcabc";
        let mut body = b"BPS1".to_vec();
        body.extend(number(3));
        body.extend(number(6));
        body.extend(number(0));
        body.extend(number(((6 - 1) << 2) | 2)); // copy 6 from a 3-byte source
        body.extend(number(0));
        let patch = with_footer(body, source, target);
        assert_eq!(patch_error(apply(source, &patch)), PatchError::OutOfRange);
    }
}
