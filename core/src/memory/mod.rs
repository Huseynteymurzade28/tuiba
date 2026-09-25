//! GBA memory map.
//!
//! The ARM7TDMI sees a flat 32-bit address space. The top byte of an address
//! selects the device; the remaining 24 bits are the offset within it. Most
//! regions are far smaller than 16 MiB and mirror across their page, which
//! is handled by masking with `SIZE - 1` (all sizes are powers of two) once
//! the bus is implemented.
//!
//! ```text
//! 00000000-00003FFF  BIOS          16 KiB   (read-only, protected)
//! 02000000-0203FFFF  EWRAM        256 KiB   (on-board, 16-bit bus)
//! 03000000-03007FFF  IWRAM         32 KiB   (on-chip, 32-bit bus)
//! 04000000-040003FE  I/O registers
//! 05000000-050003FF  Palette RAM    1 KiB
//! 06000000-06017FFF  VRAM          96 KiB
//! 07000000-070003FF  OAM            1 KiB
//! 08000000-09FFFFFF  ROM  wait-state 0
//! 0A000000-0BFFFFFF  ROM  wait-state 1   (mirror of WS0)
//! 0C000000-0DFFFFFF  ROM  wait-state 2   (mirror of WS0)
//! 0E000000-0E00FFFF  SRAM / Flash  64 KiB (8-bit bus)
//! ```

pub mod backup;
pub mod bus;
pub mod cartridge;
pub mod dma;
pub mod eeprom;
pub mod io;
pub mod timers;
pub mod video;
pub mod wait;

pub use backup::{Backup, SaveType};
pub use bus::Bus;
pub use cartridge::{Cartridge, Header};
pub use io::{Interrupt, IoRegisters};
pub use video::VideoMemory;
pub use wait::WaitStates;

/// Byte-addressable memory as seen by the CPU and DMA.
///
/// Halfword and word accessors may receive unaligned addresses and ignore
/// the low bits, as the 16- and 32-bit buses do — except where the bus is
/// 8 bits wide (SRAM), which sees the exact address. The ARM7TDMI applies
/// its own rotation rules to what a load returns.
pub trait Memory {
    /// Reads a byte.
    fn read8(&self, address: u32) -> u8;
    /// Reads a halfword from the halfword containing `address`.
    fn read16(&self, address: u32) -> u16;
    /// Reads a word from the word containing `address`.
    fn read32(&self, address: u32) -> u32;
    /// Writes a byte.
    fn write8(&mut self, address: u32, value: u8);
    /// Writes a halfword to the halfword containing `address`.
    fn write16(&mut self, address: u32, value: u16);
    /// Writes a word to the word containing `address`.
    fn write32(&mut self, address: u32, value: u32);

    /// Returns the cycles spent on accesses since the previous call and
    /// resets the count. Memories without timing report zero.
    fn take_access_cycles(&self) -> u32 {
        0
    }

    /// Reports `cycles` during which the bus was idle (the CPU was busy
    /// internally), so the cartridge prefetcher can work ahead.
    fn idle(&self, cycles: u32) {
        let _ = cycles;
    }
}

/// Size of the BIOS ROM in bytes.
pub const BIOS_SIZE: usize = 16 * 1024;
/// Size of on-board work RAM (EWRAM) in bytes.
pub const EWRAM_SIZE: usize = 256 * 1024;
/// Size of on-chip work RAM (IWRAM) in bytes.
pub const IWRAM_SIZE: usize = 32 * 1024;
/// Size of the I/O register block in bytes.
pub const IO_SIZE: usize = 0x400;
/// Size of palette RAM in bytes.
pub const PALETTE_SIZE: usize = 1024;
/// Size of video RAM in bytes. Note: **not** a power of two.
pub const VRAM_SIZE: usize = 96 * 1024;
/// Size of object attribute memory in bytes.
pub const OAM_SIZE: usize = 1024;
/// Maximum cartridge ROM size in bytes.
pub const ROM_MAX_SIZE: usize = 32 * 1024 * 1024;
/// Size of cartridge SRAM in bytes.
pub const SRAM_SIZE: usize = 64 * 1024;

/// Base address of each region (see module docs).
pub mod base {
    /// BIOS.
    pub const BIOS: u32 = 0x0000_0000;
    /// On-board work RAM.
    pub const EWRAM: u32 = 0x0200_0000;
    /// On-chip work RAM.
    pub const IWRAM: u32 = 0x0300_0000;
    /// Memory-mapped I/O registers.
    pub const IO: u32 = 0x0400_0000;
    /// Palette RAM.
    pub const PALETTE: u32 = 0x0500_0000;
    /// Video RAM.
    pub const VRAM: u32 = 0x0600_0000;
    /// Object attribute memory.
    pub const OAM: u32 = 0x0700_0000;
    /// Cartridge ROM, wait-state 0.
    pub const ROM_WS0: u32 = 0x0800_0000;
    /// Cartridge ROM, wait-state 1.
    pub const ROM_WS1: u32 = 0x0A00_0000;
    /// Cartridge ROM, wait-state 2.
    pub const ROM_WS2: u32 = 0x0C00_0000;
    /// Cartridge SRAM / Flash.
    pub const SRAM: u32 = 0x0E00_0000;
}

/// A device in the GBA address space, selected by the top byte of an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryRegion {
    /// BIOS ROM.
    Bios,
    /// On-board work RAM.
    Ewram,
    /// On-chip work RAM.
    Iwram,
    /// Memory-mapped I/O registers.
    Io,
    /// Palette RAM.
    Palette,
    /// Video RAM.
    Vram,
    /// Object attribute memory.
    Oam,
    /// Cartridge ROM (any of the three wait-state mirrors).
    Rom,
    /// Cartridge SRAM / Flash.
    Sram,
}

impl MemoryRegion {
    /// Classifies `address` by its top byte.
    ///
    /// Returns `None` for the unused gaps (`0x01`, `0x0F`–`0xFF`), which on
    /// real hardware read back as open bus.
    #[must_use]
    pub const fn from_address(address: u32) -> Option<Self> {
        match address >> 24 {
            0x00 => Some(Self::Bios),
            0x02 => Some(Self::Ewram),
            0x03 => Some(Self::Iwram),
            0x04 => Some(Self::Io),
            0x05 => Some(Self::Palette),
            0x06 => Some(Self::Vram),
            0x07 => Some(Self::Oam),
            0x08..=0x0D => Some(Self::Rom),
            0x0E | 0x0F => Some(Self::Sram),
            _ => None,
        }
    }

    /// Size of the backing storage for this region in bytes.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Self::Bios => BIOS_SIZE,
            Self::Ewram => EWRAM_SIZE,
            Self::Iwram => IWRAM_SIZE,
            Self::Io => IO_SIZE,
            Self::Palette => PALETTE_SIZE,
            Self::Vram => VRAM_SIZE,
            Self::Oam => OAM_SIZE,
            Self::Rom => ROM_MAX_SIZE,
            Self::Sram => SRAM_SIZE,
        }
    }
}

/// Helpers shared by unit tests across the memory module.
#[cfg(test)]
pub(crate) mod test_util {
    use super::cartridge::HEADER_END;

    /// Builds a minimal ROM with a valid header and the given title.
    pub(crate) fn rom_with_header(title: &str, len: usize) -> Vec<u8> {
        let mut rom = vec![0u8; len.max(HEADER_END)];
        rom[0xA0..0xA0 + title.len()].copy_from_slice(title.as_bytes());
        rom[0xAC..0xB0].copy_from_slice(b"TEST");
        rom[0xB0..0xB2].copy_from_slice(b"00");
        rom[0xB2] = 0x96;
        rom[0xBC] = 1;
        let checksum = rom[0xA0..=0xBC]
            .iter()
            .fold(0u8, |acc, &b| acc.wrapping_sub(b))
            .wrapping_sub(0x19);
        rom[0xBD] = checksum;
        rom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_region_bases() {
        assert_eq!(
            MemoryRegion::from_address(base::BIOS),
            Some(MemoryRegion::Bios)
        );
        assert_eq!(
            MemoryRegion::from_address(base::EWRAM),
            Some(MemoryRegion::Ewram)
        );
        assert_eq!(
            MemoryRegion::from_address(base::IWRAM),
            Some(MemoryRegion::Iwram)
        );
        assert_eq!(MemoryRegion::from_address(base::IO), Some(MemoryRegion::Io));
        assert_eq!(
            MemoryRegion::from_address(base::PALETTE),
            Some(MemoryRegion::Palette)
        );
        assert_eq!(
            MemoryRegion::from_address(base::VRAM),
            Some(MemoryRegion::Vram)
        );
        assert_eq!(
            MemoryRegion::from_address(base::OAM),
            Some(MemoryRegion::Oam)
        );
        assert_eq!(
            MemoryRegion::from_address(base::ROM_WS0),
            Some(MemoryRegion::Rom)
        );
        assert_eq!(
            MemoryRegion::from_address(base::ROM_WS1),
            Some(MemoryRegion::Rom)
        );
        assert_eq!(
            MemoryRegion::from_address(base::ROM_WS2),
            Some(MemoryRegion::Rom)
        );
        assert_eq!(
            MemoryRegion::from_address(base::SRAM),
            Some(MemoryRegion::Sram)
        );
    }

    #[test]
    fn classifies_offsets_within_region() {
        assert_eq!(
            MemoryRegion::from_address(0x0300_7FFC),
            Some(MemoryRegion::Iwram)
        );
        assert_eq!(
            MemoryRegion::from_address(0x09FF_FFFE),
            Some(MemoryRegion::Rom)
        );
        assert_eq!(
            MemoryRegion::from_address(0x0DFF_FFFF),
            Some(MemoryRegion::Rom)
        );
    }

    #[test]
    fn unused_pages_are_unmapped() {
        assert_eq!(MemoryRegion::from_address(0x0100_0000), None);
        assert_eq!(MemoryRegion::from_address(0x1000_0000), None);
        assert_eq!(MemoryRegion::from_address(0xFFFF_FFFF), None);
    }

    #[test]
    fn ram_sizes_are_powers_of_two() {
        for size in [
            BIOS_SIZE,
            EWRAM_SIZE,
            IWRAM_SIZE,
            PALETTE_SIZE,
            OAM_SIZE,
            SRAM_SIZE,
        ] {
            assert!(size.is_power_of_two(), "{size} is not a power of two");
        }
    }
}
