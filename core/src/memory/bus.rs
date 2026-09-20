//! The system bus: routes CPU/DMA accesses to the right device.
//!
//! All accessors take an **aligned** address for their width. The ARM7TDMI
//! forces alignment itself (and rotates misaligned loads), so that logic
//! lives in the CPU, not here.

use crate::error::{GbaError, Result};
use crate::memory::cartridge::Cartridge;
use crate::memory::io::IoRegisters;
use crate::memory::video::VideoMemory;
use crate::memory::{BIOS_SIZE, EWRAM_SIZE, IWRAM_SIZE, MemoryRegion, SRAM_SIZE};

/// Offset of an address within its 16 MiB page.
#[inline]
const fn page_offset(address: u32) -> u32 {
    address & 0x00FF_FFFF
}

#[inline]
fn get16(mem: &[u8], index: usize) -> u16 {
    u16::from_le_bytes([mem[index], mem[index + 1]])
}

#[inline]
fn get32(mem: &[u8], index: usize) -> u32 {
    u32::from_le_bytes([mem[index], mem[index + 1], mem[index + 2], mem[index + 3]])
}

#[inline]
fn set16(mem: &mut [u8], index: usize, value: u16) {
    mem[index..index + 2].copy_from_slice(&value.to_le_bytes());
}

#[inline]
fn set32(mem: &mut [u8], index: usize, value: u32) {
    mem[index..index + 4].copy_from_slice(&value.to_le_bytes());
}

/// The GBA memory bus and every memory-mapped device hanging off it.
#[derive(Debug, Clone)]
pub struct Bus {
    bios: Box<[u8]>,
    ewram: Box<[u8]>,
    iwram: Box<[u8]>,
    sram: Box<[u8]>,
    /// Memory-mapped I/O registers.
    pub io: IoRegisters,
    /// Palette RAM, VRAM and OAM.
    pub video: VideoMemory,
    /// The inserted cartridge.
    pub cartridge: Cartridge,
}

impl Bus {
    /// Creates a bus with the given cartridge and an all-zero BIOS.
    ///
    /// Without a real BIOS image the CPU must be started directly at the
    /// cartridge entry point (`0x0800_0000`). Use [`Bus::load_bios`] to
    /// install one.
    #[must_use]
    pub fn new(cartridge: Cartridge) -> Self {
        Self {
            bios: vec![0; BIOS_SIZE].into_boxed_slice(),
            ewram: vec![0; EWRAM_SIZE].into_boxed_slice(),
            iwram: vec![0; IWRAM_SIZE].into_boxed_slice(),
            sram: vec![0xFF; SRAM_SIZE].into_boxed_slice(),
            io: IoRegisters::new(),
            video: VideoMemory::new(),
            cartridge,
        }
    }

    /// Installs a BIOS image.
    ///
    /// # Errors
    ///
    /// Returns [`GbaError::BiosSize`] unless the image is exactly 16 KiB.
    pub fn load_bios(&mut self, bios: &[u8]) -> Result<()> {
        if bios.len() != BIOS_SIZE {
            return Err(GbaError::BiosSize {
                size: bios.len(),
                expected: BIOS_SIZE,
            });
        }
        self.bios.copy_from_slice(bios);
        Ok(())
    }

    /// Reads a byte.
    #[must_use]
    pub fn read8(&self, address: u32) -> u8 {
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Bios) => self.bios.get(off as usize).copied().unwrap_or(0),
            Some(MemoryRegion::Ewram) => self.ewram[off as usize & (EWRAM_SIZE - 1)],
            Some(MemoryRegion::Iwram) => self.iwram[off as usize & (IWRAM_SIZE - 1)],
            Some(MemoryRegion::Io) => self.io.read8(off),
            Some(MemoryRegion::Palette) => self.video.palette[VideoMemory::palette_index(off)],
            Some(MemoryRegion::Vram) => self.video.vram[VideoMemory::vram_index(off)],
            Some(MemoryRegion::Oam) => self.video.oam[VideoMemory::oam_index(off)],
            Some(MemoryRegion::Rom) => self.cartridge.read8(address & 0x01FF_FFFF),
            Some(MemoryRegion::Sram) => self.sram[off as usize & (SRAM_SIZE - 1)],
            None => 0,
        }
    }

    /// Reads a halfword from an even address.
    #[must_use]
    pub fn read16(&self, address: u32) -> u16 {
        debug_assert_eq!(address & 1, 0, "unaligned halfword read");
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Bios) => self
                .bios
                .get(off as usize..off as usize + 2)
                .map_or(0, |b| get16(b, 0)),
            Some(MemoryRegion::Ewram) => get16(&self.ewram, off as usize & (EWRAM_SIZE - 1)),
            Some(MemoryRegion::Iwram) => get16(&self.iwram, off as usize & (IWRAM_SIZE - 1)),
            Some(MemoryRegion::Io) => self.io.read16(off),
            Some(MemoryRegion::Palette) => {
                get16(&self.video.palette, VideoMemory::palette_index(off))
            }
            Some(MemoryRegion::Vram) => get16(&self.video.vram, VideoMemory::vram_index(off)),
            Some(MemoryRegion::Oam) => get16(&self.video.oam, VideoMemory::oam_index(off)),
            Some(MemoryRegion::Rom) => self.cartridge.read16(address & 0x01FF_FFFF),
            // SRAM is on an 8-bit bus: the byte is repeated across the halfword.
            Some(MemoryRegion::Sram) => u16::from(self.read8(address)) * 0x0101,
            None => 0,
        }
    }

    /// Reads a word from a word-aligned address.
    #[must_use]
    pub fn read32(&self, address: u32) -> u32 {
        debug_assert_eq!(address & 3, 0, "unaligned word read");
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Bios) => self
                .bios
                .get(off as usize..off as usize + 4)
                .map_or(0, |b| get32(b, 0)),
            Some(MemoryRegion::Ewram) => get32(&self.ewram, off as usize & (EWRAM_SIZE - 1)),
            Some(MemoryRegion::Iwram) => get32(&self.iwram, off as usize & (IWRAM_SIZE - 1)),
            Some(MemoryRegion::Io) => self.io.read32(off),
            Some(MemoryRegion::Palette) => {
                get32(&self.video.palette, VideoMemory::palette_index(off))
            }
            Some(MemoryRegion::Vram) => get32(&self.video.vram, VideoMemory::vram_index(off)),
            Some(MemoryRegion::Oam) => get32(&self.video.oam, VideoMemory::oam_index(off)),
            Some(MemoryRegion::Rom) => self.cartridge.read32(address & 0x01FF_FFFF),
            Some(MemoryRegion::Sram) => u32::from(self.read8(address)) * 0x0101_0101,
            None => 0,
        }
    }

    /// Writes a byte.
    ///
    /// Video memory is on a 16-bit bus: byte writes to palette RAM and VRAM
    /// (background area only) are duplicated into both bytes of the
    /// halfword, byte writes to OAM and the VRAM object area are ignored.
    pub fn write8(&mut self, address: u32, value: u8) {
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => self.ewram[off as usize & (EWRAM_SIZE - 1)] = value,
            Some(MemoryRegion::Iwram) => self.iwram[off as usize & (IWRAM_SIZE - 1)] = value,
            Some(MemoryRegion::Io) => self.io.write8(off, value),
            Some(MemoryRegion::Palette) => {
                let index = VideoMemory::palette_index(off) & !1;
                set16(&mut self.video.palette, index, u16::from(value) * 0x0101);
            }
            Some(MemoryRegion::Vram) => {
                let index = VideoMemory::vram_index(off) & !1;
                // Object tiles live above 0x10000 (0x14000 in bitmap modes;
                // the stricter bound is refined once DISPCNT is decoded).
                if index < 0x1_0000 {
                    set16(&mut self.video.vram, index, u16::from(value) * 0x0101);
                }
            }
            Some(MemoryRegion::Sram) => self.sram[off as usize & (SRAM_SIZE - 1)] = value,
            Some(MemoryRegion::Bios | MemoryRegion::Oam | MemoryRegion::Rom) | None => {}
        }
    }

    /// Writes a halfword to an even address.
    pub fn write16(&mut self, address: u32, value: u16) {
        debug_assert_eq!(address & 1, 0, "unaligned halfword write");
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => {
                set16(&mut self.ewram, off as usize & (EWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Iwram) => {
                set16(&mut self.iwram, off as usize & (IWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Io) => self.io.write16(off, value),
            Some(MemoryRegion::Palette) => {
                set16(
                    &mut self.video.palette,
                    VideoMemory::palette_index(off),
                    value,
                );
            }
            Some(MemoryRegion::Vram) => {
                set16(&mut self.video.vram, VideoMemory::vram_index(off), value);
            }
            Some(MemoryRegion::Oam) => {
                set16(&mut self.video.oam, VideoMemory::oam_index(off), value);
            }
            // 8-bit bus: only the low byte reaches the chip.
            Some(MemoryRegion::Sram) => self.write8(address, value as u8),
            Some(MemoryRegion::Bios | MemoryRegion::Rom) | None => {}
        }
    }

    /// Writes a word to a word-aligned address.
    pub fn write32(&mut self, address: u32, value: u32) {
        debug_assert_eq!(address & 3, 0, "unaligned word write");
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => {
                set32(&mut self.ewram, off as usize & (EWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Iwram) => {
                set32(&mut self.iwram, off as usize & (IWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Io) => self.io.write32(off, value),
            Some(MemoryRegion::Palette) => {
                set32(
                    &mut self.video.palette,
                    VideoMemory::palette_index(off),
                    value,
                );
            }
            Some(MemoryRegion::Vram) => {
                set32(&mut self.video.vram, VideoMemory::vram_index(off), value);
            }
            Some(MemoryRegion::Oam) => {
                set32(&mut self.video.oam, VideoMemory::oam_index(off), value);
            }
            Some(MemoryRegion::Sram) => self.write8(address, value as u8),
            Some(MemoryRegion::Bios | MemoryRegion::Rom) | None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::base;
    use crate::memory::test_util::rom_with_header;

    fn bus() -> Bus {
        let mut rom = rom_with_header("BUS", 0x200);
        rom[0x1F0..0x1F4].copy_from_slice(&[0xEF, 0xBE, 0xAD, 0xDE]);
        Bus::new(Cartridge::from_bytes(rom).unwrap())
    }

    #[test]
    fn ram_round_trips_all_widths() {
        let mut bus = bus();
        for base in [base::EWRAM, base::IWRAM] {
            bus.write32(base, 0x1234_5678);
            assert_eq!(bus.read32(base), 0x1234_5678);
            assert_eq!(bus.read16(base + 2), 0x1234);
            assert_eq!(bus.read8(base + 1), 0x56);
            bus.write8(base + 3, 0xAA);
            bus.write16(base, 0xBBCC);
            assert_eq!(bus.read32(base), 0xAA34_BBCC);
        }
    }

    #[test]
    fn work_ram_mirrors() {
        let mut bus = bus();
        bus.write16(base::EWRAM, 0xCAFE);
        assert_eq!(bus.read16(base::EWRAM + EWRAM_SIZE as u32), 0xCAFE);
        assert_eq!(bus.read16(0x02FC_0000), 0xCAFE);
        bus.write16(base::IWRAM + 4, 0xF00D);
        assert_eq!(bus.read16(base::IWRAM + IWRAM_SIZE as u32 + 4), 0xF00D);
    }

    #[test]
    fn rom_reads_through_all_wait_state_mirrors() {
        let bus = bus();
        assert_eq!(bus.read32(base::ROM_WS0 + 0x1F0), 0xDEAD_BEEF);
        assert_eq!(bus.read32(base::ROM_WS1 + 0x1F0), 0xDEAD_BEEF);
        assert_eq!(bus.read32(base::ROM_WS2 + 0x1F0), 0xDEAD_BEEF);
        assert_eq!(bus.read16(base::ROM_WS0 + 0x1F2), 0xDEAD);
        assert_eq!(bus.read8(base::ROM_WS0 + 0x1F3), 0xDE);
    }

    #[test]
    fn rom_and_bios_ignore_writes() {
        let mut bus = bus();
        bus.write32(base::ROM_WS0 + 0x1F0, 0);
        assert_eq!(bus.read32(base::ROM_WS0 + 0x1F0), 0xDEAD_BEEF);
        bus.write32(base::BIOS, 0xFFFF_FFFF);
        assert_eq!(bus.read32(base::BIOS), 0);
    }

    #[test]
    fn bios_loads_only_at_exact_size() {
        let mut bus = bus();
        assert!(matches!(
            bus.load_bios(&[0; 10]),
            Err(GbaError::BiosSize { size: 10, .. })
        ));
        let mut bios = vec![0; BIOS_SIZE];
        bios[0..4].copy_from_slice(&[0x18, 0x00, 0x00, 0xEA]); // b 0x68
        bus.load_bios(&bios).unwrap();
        assert_eq!(bus.read32(base::BIOS), 0xEA00_0018);
    }

    #[test]
    fn video_memory_byte_write_semantics() {
        let mut bus = bus();
        bus.write8(base::PALETTE + 3, 0x7C);
        assert_eq!(bus.read16(base::PALETTE + 2), 0x7C7C);

        bus.write8(base::VRAM + 0x100, 0x12);
        assert_eq!(bus.read16(base::VRAM + 0x100), 0x1212);
        bus.write8(base::VRAM + 0x1_0000, 0x34);
        assert_eq!(
            bus.read16(base::VRAM + 0x1_0000),
            0,
            "object VRAM ignores byte writes"
        );

        bus.write8(base::OAM, 0x56);
        assert_eq!(bus.read16(base::OAM), 0, "OAM ignores byte writes");
        bus.write16(base::OAM, 0x5678);
        assert_eq!(bus.read16(base::OAM), 0x5678);
    }

    #[test]
    fn vram_mirror_via_bus() {
        let mut bus = bus();
        bus.write16(base::VRAM + 0x1_0000, 0xABCD);
        assert_eq!(bus.read16(base::VRAM + 0x1_8000), 0xABCD);
        assert_eq!(bus.read16(base::VRAM + 0x2_0000 + 0x1_0000), 0xABCD);
    }

    #[test]
    fn sram_is_eight_bit_bus() {
        let mut bus = bus();
        assert_eq!(
            bus.read8(base::SRAM),
            0xFF,
            "fresh SRAM reads as erased flash"
        );
        bus.write32(base::SRAM, 0x1234_5678);
        assert_eq!(bus.read8(base::SRAM), 0x78);
        assert_eq!(bus.read16(base::SRAM), 0x7878);
        assert_eq!(bus.read32(base::SRAM), 0x7878_7878);
    }

    #[test]
    fn io_is_routed() {
        let mut bus = bus();
        bus.write16(base::IO + crate::memory::io::reg::DISPCNT, 0x0F03);
        assert_eq!(
            bus.read16(base::IO + crate::memory::io::reg::DISPCNT),
            0x0F03
        );
        assert_eq!(
            bus.read16(base::IO + crate::memory::io::reg::KEYINPUT),
            0x03FF
        );
    }

    #[test]
    fn unmapped_pages_read_zero_and_swallow_writes() {
        let mut bus = bus();
        bus.write32(0x0100_0000, 0xFFFF_FFFF);
        assert_eq!(bus.read32(0x0100_0000), 0);
        assert_eq!(bus.read8(0xF000_0000), 0);
    }
}
