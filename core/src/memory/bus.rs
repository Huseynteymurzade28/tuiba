//! The system bus: routes CPU/DMA accesses to the right device.
//!
//! All accessors take an **aligned** address for their width. The ARM7TDMI
//! forces alignment itself (and rotates misaligned loads), so that logic
//! lives in the CPU, not here.

use std::cell::Cell;

use crate::error::{GbaError, Result};
use crate::memory::backup::Backup;
use crate::memory::cartridge::Cartridge;
use crate::memory::io::{IoRegisters, reg};
use crate::memory::video::VideoMemory;
use crate::memory::wait::WaitStates;
use crate::memory::{BIOS_SIZE, EWRAM_SIZE, IWRAM_SIZE, Memory, MemoryRegion};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Bus {
    bios: Box<[u8]>,
    ewram: Box<[u8]>,
    iwram: Box<[u8]>,
    /// Save memory at `0x0E00_0000`.
    pub backup: Backup,
    /// Memory-mapped I/O registers.
    pub io: IoRegisters,
    /// Palette RAM, VRAM and OAM.
    pub video: VideoMemory,
    /// The inserted cartridge.
    pub cartridge: Cartridge,
    /// Access costs, decoded from `WAITCNT`.
    pub wait: WaitStates,
    /// Cycles spent on accesses since the last [`Memory::take_access_cycles`].
    /// Reads take `&self`, hence the cells.
    access_cycles: Cell<u32>,
    /// Address just past the previous access, for sequential detection.
    next_sequential: Cell<u32>,
    /// Address just past the previous *cartridge* access: where the
    /// prefetcher continues from.
    rom_next: Cell<u32>,
    /// Cycles the prefetcher has had to itself since the last cartridge
    /// access; every `S` cycles buffer one more halfword.
    prefetch_cycles: Cell<u32>,
}

/// Halfwords the prefetch buffer can hold.
const PREFETCH_DEPTH: u32 = 8;

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
            backup: Backup::for_type(cartridge.save_type()),
            io: IoRegisters::new(),
            video: VideoMemory::new(),
            cartridge,
            wait: WaitStates::default(),
            access_cycles: Cell::new(0),
            next_sequential: Cell::new(u32::MAX),
            rom_next: Cell::new(u32::MAX),
            prefetch_cycles: Cell::new(0),
        }
    }

    /// Charges an access of `width` bytes at `address`.
    ///
    /// An access is sequential when it continues directly from the
    /// previous one; the cartridge additionally restarts at every 128 KiB
    /// boundary. With prefetch enabled, the cartridge keeps fetching
    /// halfwords on its own whenever the bus is busy elsewhere, and an
    /// access that continues from the last cartridge access takes one
    /// cycle per halfword already buffered.
    #[inline]
    fn account(&self, address: u32, width: u32) {
        let page = self.wait.page(address);
        let continues = |from: u32| address == from && address & 0x1_FFFF != 0;
        let cycles = if (0x08..=0x0D).contains(&(address >> 24)) {
            let cycles = if self.wait.prefetch && continues(self.rom_next.get()) {
                let halfwords = width.div_ceil(2);
                let buffered =
                    (self.prefetch_cycles.get() / u32::from(page.s16)).min(PREFETCH_DEPTH);
                let hit = buffered.min(halfwords);
                hit + (halfwords - hit) * u32::from(page.s16)
            } else {
                page.cycles(width, continues(self.next_sequential.get()))
            };
            self.rom_next.set(address.wrapping_add(width));
            self.prefetch_cycles.set(0);
            cycles
        } else {
            let cycles = page.cycles(width, address == self.next_sequential.get());
            self.idle(cycles);
            cycles
        };
        self.access_cycles.set(self.access_cycles.get() + cycles);
        self.next_sequential.set(address.wrapping_add(width));
    }

    /// Re-decodes the timing table after an I/O write that may have
    /// touched `WAITCNT`.
    fn after_io_write(&mut self, offset: u32, width: u32) {
        if offset < reg::WAITCNT + 2 && offset + width > reg::WAITCNT {
            self.wait = WaitStates::from_waitcnt(self.io.read16(reg::WAITCNT));
        }
    }

    /// Writes a word into the BIOS region. Used to install the HLE
    /// exception stubs; software cannot write here.
    pub fn poke_bios(&mut self, address: u32, value: u32) {
        let i = address as usize;
        if i + 4 <= BIOS_SIZE {
            self.bios[i..i + 4].copy_from_slice(&value.to_le_bytes());
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

    /// Whether a ROM-area address reaches the EEPROM chip instead of the
    /// ROM: the whole `0x0D` page for carts up to 16 MiB, only its top
    /// 256 bytes for larger ones.
    #[must_use]
    pub fn is_eeprom_address(&self, address: u32) -> bool {
        address >> 24 == 0x0D
            && self.backup.eeprom().is_some()
            && (self.cartridge.rom().len() <= 0x100_0000 || address & 0x00FF_FFFF >= 0x00FF_FF00)
    }

    /// Lets the EEPROM learn its size from a DMA transfer length.
    pub fn hint_eeprom_transfer(&mut self, address: u32, units: u32) {
        if self.is_eeprom_address(address)
            && let Some(eeprom) = self.backup.eeprom_mut()
        {
            eeprom.hint_transfer_len(units);
        }
    }

    fn eeprom_read(&self) -> u16 {
        self.backup.eeprom().map_or(1, super::eeprom::Eeprom::read)
    }

    fn eeprom_write(&mut self, value: u16) {
        if let Some(eeprom) = self.backup.eeprom_mut() {
            eeprom.write(value);
        }
    }
}

impl Memory for Bus {
    /// Reads a byte.
    fn read8(&self, address: u32) -> u8 {
        self.account(address, 1);
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Bios) => self.bios.get(off as usize).copied().unwrap_or(0),
            Some(MemoryRegion::Ewram) => self.ewram[off as usize & (EWRAM_SIZE - 1)],
            Some(MemoryRegion::Iwram) => self.iwram[off as usize & (IWRAM_SIZE - 1)],
            Some(MemoryRegion::Io) => self.io.read8(off),
            Some(MemoryRegion::Palette) => self.video.palette[VideoMemory::palette_index(off)],
            Some(MemoryRegion::Vram) => self.video.vram[VideoMemory::vram_index(off)],
            Some(MemoryRegion::Oam) => self.video.oam[VideoMemory::oam_index(off)],
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => self.eeprom_read() as u8,
            Some(MemoryRegion::Rom) => self.cartridge.read8(address & 0x01FF_FFFF),
            Some(MemoryRegion::Sram) => self.backup.read(off),
            None => 0,
        }
    }

    /// Reads a halfword from an even address.
    fn read16(&self, exact: u32) -> u16 {
        let address = exact & !1;
        self.account(address, 2);
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
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => self.eeprom_read(),
            Some(MemoryRegion::Rom) => self.cartridge.read16(address & 0x01FF_FFFF),
            // SRAM is on an 8-bit bus: the addressed byte is repeated
            // across the halfword.
            Some(MemoryRegion::Sram) => u16::from(self.backup.read(page_offset(exact))) * 0x0101,
            None => 0,
        }
    }

    /// Reads a word from a word-aligned address.
    fn read32(&self, exact: u32) -> u32 {
        let address = exact & !3;
        self.account(address, 4);
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
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => {
                u32::from(self.eeprom_read()) | u32::from(self.eeprom_read()) << 16
            }
            Some(MemoryRegion::Rom) => self.cartridge.read32(address & 0x01FF_FFFF),
            Some(MemoryRegion::Sram) => {
                u32::from(self.backup.read(page_offset(exact))) * 0x0101_0101
            }
            None => 0,
        }
    }

    /// Writes a byte.
    ///
    /// Video memory is on a 16-bit bus: byte writes to palette RAM and VRAM
    /// (background area only) are duplicated into both bytes of the
    /// halfword, byte writes to OAM and the VRAM object area are ignored.
    fn write8(&mut self, address: u32, value: u8) {
        self.account(address, 1);
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => self.ewram[off as usize & (EWRAM_SIZE - 1)] = value,
            Some(MemoryRegion::Iwram) => self.iwram[off as usize & (IWRAM_SIZE - 1)] = value,
            Some(MemoryRegion::Io) => {
                self.io.write8(off, value);
                self.after_io_write(off, 1);
            }
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
            Some(MemoryRegion::Sram) => self.backup.write(off, value),
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => {
                self.eeprom_write(u16::from(value));
            }
            Some(MemoryRegion::Bios | MemoryRegion::Oam | MemoryRegion::Rom) | None => {}
        }
    }

    /// Writes a halfword to an even address.
    fn write16(&mut self, exact: u32, value: u16) {
        let address = exact & !1;
        self.account(address, 2);
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => {
                set16(&mut self.ewram, off as usize & (EWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Iwram) => {
                set16(&mut self.iwram, off as usize & (IWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Io) => {
                self.io.write16(off, value);
                self.after_io_write(off, 2);
            }
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
            // 8-bit bus: one byte reaches the chip, the one the store
            // would have put at the exact address.
            Some(MemoryRegion::Sram) => {
                let byte = value.rotate_right((exact & 1) * 8) as u8;
                self.backup.write(page_offset(exact), byte);
            }
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => self.eeprom_write(value),
            Some(MemoryRegion::Bios | MemoryRegion::Rom) | None => {}
        }
    }

    /// Writes a word to a word-aligned address.
    fn write32(&mut self, exact: u32, value: u32) {
        let address = exact & !3;
        self.account(address, 4);
        let off = page_offset(address);
        match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram) => {
                set32(&mut self.ewram, off as usize & (EWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Iwram) => {
                set32(&mut self.iwram, off as usize & (IWRAM_SIZE - 1), value);
            }
            Some(MemoryRegion::Io) => {
                self.io.write32(off, value);
                self.after_io_write(off, 4);
            }
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
            Some(MemoryRegion::Sram) => {
                let byte = value.rotate_right((exact & 3) * 8) as u8;
                self.backup.write(page_offset(exact), byte);
            }
            Some(MemoryRegion::Rom) if self.is_eeprom_address(address) => {
                self.eeprom_write(value as u16);
                self.eeprom_write((value >> 16) as u16);
            }
            Some(MemoryRegion::Bios | MemoryRegion::Rom) | None => {}
        }
    }

    fn take_access_cycles(&self) -> u32 {
        self.access_cycles.replace(0)
    }

    fn idle(&self, cycles: u32) {
        // The buffer is at most PREFETCH_DEPTH halfwords deep; the cap
        // uses the slowest possible S timing so no setting overflows it.
        const CAP: u32 = PREFETCH_DEPTH * 9;
        self.prefetch_cycles
            .set((self.prefetch_cycles.get() + cycles).min(CAP));
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
    fn accesses_are_charged_by_region_and_sequence() {
        let mut bus = bus();
        assert_eq!(bus.take_access_cycles(), 0);

        bus.read32(base::IWRAM);
        bus.read8(base::IWRAM + 4);
        assert_eq!(bus.take_access_cycles(), 2, "IWRAM: one cycle each");
        bus.write32(base::EWRAM, 0);
        assert_eq!(
            bus.take_access_cycles(),
            6,
            "EWRAM word: two 3-cycle halves"
        );
        bus.read16(base::PALETTE);
        bus.read32(base::VRAM);
        assert_eq!(bus.take_access_cycles(), 1 + 2);

        // ROM (WS0 default 4/2 waits): first access non-sequential, the
        // next one continues where it left off.
        bus.read16(base::ROM_WS0 + 0x100);
        assert_eq!(bus.take_access_cycles(), 5);
        bus.read16(base::ROM_WS0 + 0x102);
        assert_eq!(bus.take_access_cycles(), 3);
        bus.read32(base::ROM_WS0 + 0x104);
        assert_eq!(bus.take_access_cycles(), 6, "sequential word: S + S");
        bus.read32(base::ROM_WS0 + 0x200);
        assert_eq!(bus.take_access_cycles(), 8, "jump: N + S");
        // Writes elsewhere break the sequence.
        bus.read16(base::ROM_WS0 + 0x204);
        bus.write16(base::IWRAM, 0);
        bus.read16(base::ROM_WS0 + 0x206);
        assert_eq!(bus.take_access_cycles(), 3 + 1 + 5);
        // Every 128 KiB the cartridge restarts its address latch.
        bus.read16(base::ROM_WS0 + 0x1_FFFE);
        bus.read16(base::ROM_WS0 + 0x2_0000);
        assert_eq!(bus.take_access_cycles(), 5 + 5);

        bus.read8(base::SRAM);
        assert_eq!(bus.take_access_cycles(), 5);
        assert_eq!(bus.take_access_cycles(), 0, "taking resets");
    }

    #[test]
    fn prefetch_serves_sequential_fetches_after_idle_time() {
        let mut bus = bus();
        // Without prefetch, leaving the cartridge makes the next access N.
        bus.read16(base::ROM_WS0 + 0x100);
        bus.read32(base::IWRAM);
        bus.take_access_cycles();
        bus.read16(base::ROM_WS0 + 0x102);
        assert_eq!(bus.take_access_cycles(), 5);

        // Prefetch on, WS0 = 4/2.
        bus.write16(base::IO + reg::WAITCNT, 0x4317);
        bus.read16(base::ROM_WS0 + 0x200);
        bus.take_access_cycles();
        bus.read32(base::IWRAM); // 1 idle cycle: not enough for a halfword
        bus.take_access_cycles();
        bus.read16(base::ROM_WS0 + 0x202);
        assert_eq!(
            bus.take_access_cycles(),
            2,
            "continues, but nothing buffered yet"
        );
        bus.read16(base::ROM_WS0 + 0x204);
        assert_eq!(bus.take_access_cycles(), 2, "back to back: plain S");
        bus.idle(2);
        bus.read16(base::ROM_WS0 + 0x206);
        assert_eq!(bus.take_access_cycles(), 1, "one buffered halfword");
        bus.idle(100);
        bus.read32(base::ROM_WS0 + 0x208);
        assert_eq!(bus.take_access_cycles(), 2, "word from the buffer");
        bus.read32(base::ROM_WS0 + 0x20C);
        assert_eq!(bus.take_access_cycles(), 4, "buffer drained: S + S");
        bus.idle(100);
        bus.read16(base::ROM_WS0 + 0x400);
        assert_eq!(bus.take_access_cycles(), 4, "a jump is N, buffer discarded");
        bus.idle(100);
        for i in 0..8 {
            bus.read16(base::ROM_WS0 + 0x402 + i * 2);
        }
        assert_eq!(
            bus.take_access_cycles(),
            1 + 7 * 2,
            "the buffer holds 8 halfwords"
        );
    }

    #[test]
    fn waitcnt_retimes_the_cartridge() {
        let mut bus = bus();
        bus.write16(base::IO + reg::WAITCNT, 0x4317);
        bus.take_access_cycles();
        assert!(bus.wait.prefetch);
        bus.read16(base::ROM_WS0);
        assert_eq!(bus.take_access_cycles(), 4);
        bus.read16(base::ROM_WS0 + 2);
        assert_eq!(bus.take_access_cycles(), 2);
        bus.read8(base::SRAM);
        assert_eq!(bus.take_access_cycles(), 9);
        // Byte and word writes reach it too.
        bus.write8(base::IO + reg::WAITCNT, 0);
        assert_eq!(bus.wait.page(base::ROM_WS0).n16, 5);
        bus.write32(base::IO + reg::WAITCNT, 0x0000_0004);
        assert_eq!(bus.wait.page(base::ROM_WS0).n16, 4);
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
        // Wider stores put the byte meant for the exact address there.
        bus.write16(base::SRAM + 1, 0xAABB);
        assert_eq!(bus.read8(base::SRAM + 1), 0xAA);
        bus.write32(base::SRAM + 2, 0xAABB_CCDD);
        assert_eq!(bus.read8(base::SRAM + 2), 0xBB);
        assert_eq!(bus.read8(base::SRAM + 3), 0xFF, "one byte, not four");
        assert_eq!(bus.read32(base::SRAM + 2), 0xBBBB_BBBB);
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
