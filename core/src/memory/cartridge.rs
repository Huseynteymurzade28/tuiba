//! Cartridge ROM loading and header parsing.

use std::path::Path;
use std::sync::Arc;

use crate::error::{GbaError, Result};
use crate::memory::ROM_MAX_SIZE;
use crate::memory::backup::SaveType;

/// FNV-1a over a ROM image: enough to tell two cartridges apart, and
/// cheap enough to take once while loading one.
fn fingerprint(rom: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    rom.iter().fold(OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// The empty ROM a deserialized cartridge starts with.
fn no_rom() -> Arc<[u8]> {
    Arc::from(&[][..])
}

/// Offset of the cartridge header within the ROM.
pub const HEADER_OFFSET: usize = 0xA0;
/// Minimum ROM size: the header must be present in full.
pub const HEADER_END: usize = 0xC0;

/// Parsed cartridge header (bytes `0xA0..0xC0` of the ROM).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Header {
    /// Game title, up to 12 ASCII characters, trailing NULs stripped.
    pub title: String,
    /// Four-character game code, e.g. `"AXVE"`.
    pub game_code: String,
    /// Two-character maker code, e.g. `"01"` for Nintendo.
    pub maker_code: String,
    /// Software version number.
    pub version: u8,
    /// Header checksum as stored in the ROM.
    pub checksum: u8,
    /// Whether the fixed byte at `0xB2` has the required value `0x96` and
    /// the stored checksum matches the computed one.
    ///
    /// Real hardware refuses to boot ROMs that fail this check, but the
    /// emulator does not: many homebrew ROMs are never run through
    /// `gbafix` and are otherwise perfectly valid.
    pub valid: bool,
}

impl Header {
    /// Parses the header from the start of a ROM image, which may be just
    /// the first [`HEADER_END`] bytes of the file. Returns `None` when
    /// `rom` is too short to hold a header.
    #[must_use]
    pub fn from_prefix(rom: &[u8]) -> Option<Self> {
        (rom.len() >= HEADER_END).then(|| Self::parse(rom))
    }

    /// Parses the header from the first `HEADER_END` bytes of a ROM.
    ///
    /// # Panics
    ///
    /// Panics if `rom` is shorter than [`HEADER_END`]; callers must check
    /// the length first.
    fn parse(rom: &[u8]) -> Self {
        let ascii = |range: std::ops::Range<usize>| -> String {
            rom[range]
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| char::from(b))
                .collect()
        };

        let computed = rom[0xA0..=0xBC]
            .iter()
            .fold(0u8, |acc, &b| acc.wrapping_sub(b))
            .wrapping_sub(0x19);
        let checksum = rom[0xBD];

        Self {
            title: ascii(0xA0..0xAC),
            game_code: ascii(0xAC..0xB0),
            maker_code: ascii(0xB0..0xB2),
            version: rom[0xBC],
            checksum,
            valid: rom[0xB2] == 0x96 && checksum == computed,
        }
    }
}

/// A loaded cartridge: ROM image plus parsed header.
///
/// The ROM is shared rather than copied: cloning a [`Cartridge`] — which
/// a save state does, through [`Gba::snapshot`](crate::Gba::snapshot) —
/// must not duplicate up to 32 MiB that nothing ever writes to.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Cartridge {
    /// FNV-1a over the ROM image, taken once at load.
    fingerprint: u64,
    /// Not part of a save state: the ROM is the game the state belongs
    /// to, and reattached from the running cartridge when one is loaded
    /// (see [`Snapshot::from_bytes`](crate::Snapshot::from_bytes)).
    #[serde(skip, default = "no_rom")]
    rom: Arc<[u8]>,
    header: Header,
    save_type: SaveType,
}

impl Cartridge {
    /// Wraps a ROM image, validating its size and parsing the header.
    ///
    /// # Errors
    ///
    /// Returns [`GbaError::RomTooSmall`] if the image cannot hold a header,
    /// or [`GbaError::RomTooLarge`] if it exceeds the 32 MiB address space.
    pub fn from_bytes(rom: Vec<u8>) -> Result<Self> {
        let size = rom.len();
        if size < HEADER_END {
            return Err(GbaError::RomTooSmall {
                size,
                min: HEADER_END,
            });
        }
        if size > ROM_MAX_SIZE {
            return Err(GbaError::RomTooLarge {
                size,
                max: ROM_MAX_SIZE,
            });
        }
        let header = Header::parse(&rom);
        let save_type = SaveType::detect(&rom);
        let fingerprint = fingerprint(&rom);
        Ok(Self {
            fingerprint,
            rom: rom.into(),
            header,
            save_type,
        })
    }

    /// Reads a ROM image from disk.
    ///
    /// # Errors
    ///
    /// Returns [`GbaError::RomIo`] on I/O failure, otherwise the same errors
    /// as [`Cartridge::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// The parsed cartridge header.
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The backup chip type detected from the ROM.
    #[must_use]
    pub fn save_type(&self) -> SaveType {
        self.save_type
    }

    /// Identifies the ROM image, for telling whether a save state was
    /// taken from this cartridge.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The ROM, shared rather than copied.
    pub(crate) fn shared_rom(&self) -> Arc<[u8]> {
        Arc::clone(&self.rom)
    }

    /// Puts a ROM back into a cartridge that was deserialized without
    /// one. The caller has checked that it is the right ROM.
    pub(crate) fn attach_rom(&mut self, rom: Arc<[u8]>) {
        self.rom = rom;
    }

    /// Raw ROM contents.
    #[must_use]
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// Reads a byte at `offset` from the start of the ROM.
    ///
    /// Offsets past the end of the image return the open-bus pattern the
    /// real cartridge bus produces: each halfword reads as its own
    /// (halfword-)address, truncated to 16 bits.
    #[inline]
    #[must_use]
    pub fn read8(&self, offset: u32) -> u8 {
        match self.rom.get(offset as usize) {
            Some(&byte) => byte,
            None => (Self::open_bus(offset) >> ((offset & 1) * 8)) as u8,
        }
    }

    /// Reads a little-endian halfword at the (even) `offset`.
    #[inline]
    #[must_use]
    pub fn read16(&self, offset: u32) -> u16 {
        let off = offset as usize;
        match self.rom.get(off..off + 2) {
            Some(bytes) => u16::from_le_bytes([bytes[0], bytes[1]]),
            None => Self::open_bus(offset),
        }
    }

    /// Reads a little-endian word at the (word-aligned) `offset`.
    #[inline]
    #[must_use]
    pub fn read32(&self, offset: u32) -> u32 {
        let off = offset as usize;
        if let Some(bytes) = self.rom.get(off..off + 4) {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            let lo = u32::from(Self::open_bus(offset));
            let hi = u32::from(Self::open_bus(offset.wrapping_add(2)));
            lo | (hi << 16)
        }
    }

    /// Open-bus value for the halfword containing `offset`.
    #[inline]
    const fn open_bus(offset: u32) -> u16 {
        ((offset >> 1) & 0xFFFF) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::test_util::rom_with_header;

    #[test]
    fn parses_header_fields() {
        let cart = Cartridge::from_bytes(rom_with_header("HELLO", 0x100)).unwrap();
        let h = cart.header();
        assert_eq!(h.title, "HELLO");
        assert_eq!(h.game_code, "TEST");
        assert_eq!(h.maker_code, "00");
        assert_eq!(h.version, 1);
        assert!(h.valid);
    }

    #[test]
    fn header_from_prefix_needs_the_whole_header() {
        let rom = rom_with_header("PREFIX", 0x100);
        assert_eq!(
            Header::from_prefix(&rom[..HEADER_END]).unwrap().title,
            "PREFIX"
        );
        assert_eq!(Header::from_prefix(&rom[..HEADER_END - 1]), None);
    }

    #[test]
    fn detects_bad_checksum_without_rejecting() {
        let mut rom = rom_with_header("BAD", 0x100);
        rom[0xBD] ^= 0xFF;
        let cart = Cartridge::from_bytes(rom).unwrap();
        assert!(!cart.header().valid);
    }

    #[test]
    fn rejects_undersized_rom() {
        assert!(matches!(
            Cartridge::from_bytes(vec![0; 0x10]),
            Err(GbaError::RomTooSmall { size: 0x10, .. })
        ));
    }

    #[test]
    fn rejects_oversized_rom() {
        assert!(matches!(
            Cartridge::from_bytes(vec![0; ROM_MAX_SIZE + 1]),
            Err(GbaError::RomTooLarge { .. })
        ));
    }

    #[test]
    fn reads_little_endian() {
        let mut rom = rom_with_header("LE", 0x100);
        rom[0xF0..0xF4].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        let cart = Cartridge::from_bytes(rom).unwrap();
        assert_eq!(cart.read8(0xF0), 0x78);
        assert_eq!(cart.read16(0xF0), 0x5678);
        assert_eq!(cart.read32(0xF0), 0x1234_5678);
    }

    #[test]
    fn out_of_range_reads_open_bus() {
        let cart = Cartridge::from_bytes(rom_with_header("OB", 0x100)).unwrap();
        assert_eq!(cart.read16(0x1000), 0x0800);
        assert_eq!(cart.read32(0x1000), 0x0801_0800);
        assert_eq!(cart.read8(0x1000), 0x00);
        assert_eq!(cart.read8(0x1001), 0x08);
    }
}
