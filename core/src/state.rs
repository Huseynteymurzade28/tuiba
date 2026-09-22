//! Save states: the whole machine, frozen and thawed.
//!
//! A [`Snapshot`] is every piece of state the emulator steps — CPU
//! registers, the memories, the I/O block with its timers, DMA and sound
//! unit, the PPU and backup memory — captured at a frame boundary. The
//! cartridge ROM rides along shared rather than copied (see
//! [`Cartridge`](crate::Cartridge)), so a snapshot costs roughly the
//! size of RAM plus the framebuffer, not the size of the game.
//!
//! Restoring is a whole-machine replacement, not a merge: whatever the
//! running system had becomes the snapshot's, down to the pending
//! interrupt flags, so a restored state continues exactly where it was
//! taken rather than approximately.

use crate::error::{GbaError, Result};
use crate::gba::Gba;
use crate::memory::Cartridge;
use crate::ppu::Framebuffer;

/// First bytes of a state file, so one can be recognised (and, more
/// usefully, so something that is not one can be rejected before it is
/// decoded).
const MAGIC: [u8; 8] = *b"TUIBAST\0";

/// Version of the serialized layout. Bumped whenever the state a build
/// writes stops being readable by the same code — a field added to the
/// emulator is enough, since postcard's encoding is positional.
const FORMAT_VERSION: u16 = 1;

/// Magic, version and cartridge fingerprint, ahead of the payload.
const HEADER_LEN: usize = MAGIC.len() + 2 + 8;

/// A frozen machine, taken by [`Gba::snapshot`].
#[derive(Debug, Clone)]
pub struct Snapshot {
    gba: Gba,
}

impl Snapshot {
    /// The frame that was on screen when the snapshot was taken, for a
    /// frontend that previews its save states.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        self.gba.framebuffer()
    }

    /// The title of the cartridge the snapshot was taken from. A state
    /// only means anything to the game it came from, so a frontend that
    /// stores several has to be able to tell them apart.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.gba.bus.cartridge.header().title
    }

    /// Serializes the snapshot for writing to a file.
    ///
    /// The cartridge ROM is not included — it is the game the state
    /// belongs to, and the player already has it. What goes in instead
    /// is its fingerprint, so [`Snapshot::from_bytes`] can refuse a
    /// state that belongs to another cartridge.
    ///
    /// # Panics
    ///
    /// Panics if the machine cannot be encoded, which would mean a state
    /// type in the core that serde cannot represent — a bug here, not
    /// something a caller can cause.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + 1024);
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.gba.bus.cartridge.fingerprint().to_le_bytes());
        postcard::to_extend(&self.gba, bytes).expect("a machine is always encodable")
    }

    /// Reads back what [`Snapshot::to_bytes`] wrote, putting `cartridge`'s
    /// ROM behind it.
    ///
    /// # Errors
    ///
    /// [`GbaError::StateFormat`] if the bytes are not a state file,
    /// [`GbaError::StateVersion`] if they are one this build does not
    /// read, [`GbaError::StateCartridge`] if they belong to a different
    /// cartridge, and [`GbaError::StateCorrupt`] if the payload does not
    /// decode.
    pub fn from_bytes(bytes: &[u8], cartridge: &Cartridge) -> Result<Self> {
        let header = bytes.get(..HEADER_LEN).ok_or(GbaError::StateFormat)?;
        if header[..MAGIC.len()] != MAGIC {
            return Err(GbaError::StateFormat);
        }
        let version = u16::from_le_bytes([header[8], header[9]]);
        if version != FORMAT_VERSION {
            return Err(GbaError::StateVersion {
                found: version,
                expected: FORMAT_VERSION,
            });
        }
        let fingerprint = header[10..HEADER_LEN]
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| GbaError::StateFormat)?;
        if fingerprint != cartridge.fingerprint() {
            return Err(GbaError::StateCartridge);
        }
        let mut gba: Gba = postcard::from_bytes(&bytes[HEADER_LEN..])
            .map_err(|err| GbaError::StateCorrupt(err.to_string()))?;
        gba.bus.cartridge.attach_rom(cartridge.shared_rom());
        Ok(Self { gba })
    }
}

impl Gba {
    /// Freezes the machine.
    ///
    /// Cheap enough to call between frames: the ROM is shared with the
    /// running system, and what is copied is RAM-sized.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot { gba: self.clone() }
    }

    /// Thaws a snapshot back into this machine.
    ///
    /// Buffered audio is dropped rather than restored: the samples in
    /// flight belong to the moment the snapshot replaced, and playing
    /// them after the jump is heard as a click. The frontend's own queue
    /// wants the same treatment.
    pub fn restore(&mut self, snapshot: &Snapshot) {
        self.clone_from(&snapshot.gba);
        self.clear_audio();
    }
}

#[cfg(test)]
mod tests {
    use crate::Cartridge;
    use crate::Gba;
    use crate::GbaError;
    use crate::Memory;
    use crate::SCREEN_WIDTH;
    use crate::Snapshot;
    use crate::cpu::registers::PC;
    use crate::memory::cartridge::HEADER_END;
    use crate::memory::io::reg;

    /// The smallest cartridge the loader accepts: an entry branch into an
    /// infinite loop and a title. Enough to step a machine that has state
    /// worth freezing.
    fn minimal_rom() -> Vec<u8> {
        let mut rom = vec![0u8; HEADER_END];
        rom[0..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes()); // b .
        rom[0xA0..0xAC].copy_from_slice(b"SNAPSHOT    ");
        rom
    }

    fn booted() -> Gba {
        Gba::new(Cartridge::from_bytes(minimal_rom()).unwrap())
    }

    /// Puts something on screen: a mode-3 bitmap with a gradient and one
    /// sprite, so the PPU actually composes pixels every line — the work
    /// whose scratch buffers a state file leaves out.
    fn draw_a_scene(gba: &mut Gba) {
        // Mode 3, BG2 and OBJ on, 1D sprite mapping.
        gba.bus.io.write16(reg::DISPCNT, 0x1443);
        for i in 0..SCREEN_WIDTH * 16 {
            gba.bus.video.vram[i * 2] = (i & 0xFF) as u8;
            gba.bus.video.vram[i * 2 + 1] = ((i >> 3) & 0x7F) as u8;
        }
        // One 16x16 sprite at (40, 30), tile 0, palette 0.
        gba.bus.video.oam[0..2].copy_from_slice(&30u16.to_le_bytes());
        gba.bus.video.oam[2..4].copy_from_slice(&(0x4000 | 0x0028_u16).to_le_bytes());
        gba.bus.video.oam[4..6].copy_from_slice(&0u16.to_le_bytes());
        for (i, byte) in gba.bus.video.vram[0x1_0000..0x1_0400]
            .iter_mut()
            .enumerate()
        {
            *byte = (i % 0xFF) as u8;
        }
        for (i, byte) in gba.bus.video.palette.iter_mut().enumerate() {
            *byte = (i * 3 % 0xFF) as u8;
        }
    }

    #[test]
    fn restoring_rewinds_memory() {
        let mut gba = booted();
        gba.run_frame();
        gba.bus.write32(0x0200_0000, 0xDEAD_BEEF);
        let snapshot = gba.snapshot();

        for _ in 0..4 {
            gba.run_frame();
        }
        gba.bus.write32(0x0200_0000, 0);

        gba.restore(&snapshot);
        assert_eq!(gba.bus.read32(0x0200_0000), 0xDEAD_BEEF);
    }

    /// The point of a save state: what follows it is the same run, not a
    /// similar one. Two machines from the same snapshot must agree on the
    /// screen and the program counter after the same number of frames.
    #[test]
    fn a_restored_machine_runs_the_same_frames_again() {
        let mut gba = booted();
        gba.run_frame();
        let snapshot = gba.snapshot();

        let mut reference = gba.clone();
        for _ in 0..8 {
            reference.run_frame();
        }

        gba.restore(&snapshot);
        for _ in 0..8 {
            gba.run_frame();
        }
        assert_eq!(gba.framebuffer().pixels(), reference.framebuffer().pixels());
        assert_eq!(gba.cpu.regs.get(PC), reference.cpu.regs.get(PC));
    }

    #[test]
    fn a_snapshot_shares_the_rom_rather_than_copying_it() {
        let gba = booted();
        let snapshot = gba.snapshot();
        assert!(std::ptr::eq(
            gba.bus.cartridge.rom(),
            snapshot.gba.bus.cartridge.rom()
        ));
    }

    #[test]
    fn backup_memory_is_part_of_the_state() {
        let mut gba = booted();
        gba.load_save_data(&vec![0x11; 0x8000]);
        let snapshot = gba.snapshot();
        gba.load_save_data(&vec![0x22; 0x8000]);
        gba.restore(&snapshot);
        assert_eq!(gba.save_data()[0], 0x11);
    }

    #[test]
    fn a_snapshot_knows_which_cartridge_it_came_from() {
        let gba = booted();
        assert_eq!(gba.snapshot().title().trim(), "SNAPSHOT");
    }

    /// A state written and read back is the same machine, and keeps
    /// running as the original would have — from the very first frame,
    /// which is where the PPU scratch left out of the file would show up
    /// if anything read it before writing it.
    #[test]
    fn a_state_survives_a_round_trip_through_bytes() {
        let cartridge = Cartridge::from_bytes(minimal_rom()).unwrap();
        let mut gba = Gba::new(cartridge.clone());
        draw_a_scene(&mut gba);
        gba.run_frame();
        gba.bus.write32(0x0300_0000, 0x0BAD_F00D);
        assert!(
            gba.framebuffer()
                .pixels()
                .iter()
                .any(|&p| p != gba.framebuffer().pixels()[0]),
            "the scene should put something on screen for the comparison to mean anything"
        );
        let bytes = gba.snapshot().to_bytes();

        // The same machine, never written out: what the file has to
        // reproduce, frame for frame.
        let mut in_memory = gba.clone();

        let mut thawed = Gba::new(cartridge.clone());
        thawed.restore(&Snapshot::from_bytes(&bytes, &cartridge).unwrap());
        assert_eq!(thawed.bus.read32(0x0300_0000), 0x0BAD_F00D);

        for frame in 1..=8 {
            in_memory.run_frame();
            thawed.run_frame();
            assert_eq!(
                thawed.framebuffer().pixels(),
                in_memory.framebuffer().pixels(),
                "frame {frame} after restoring differs from the machine that stayed in memory"
            );
        }
        assert_eq!(thawed.cpu.regs.get(PC), in_memory.cpu.regs.get(PC));
    }

    /// The ROM is not in the file, so a state is small next to the game
    /// it belongs to.
    #[test]
    fn a_state_does_not_carry_the_rom() {
        let mut rom = minimal_rom();
        rom.resize(4 * 1024 * 1024, 0x5A);
        let cartridge = Cartridge::from_bytes(rom).unwrap();
        let bytes = Gba::new(cartridge).snapshot().to_bytes();
        assert!(
            bytes.len() < 1024 * 1024,
            "a state should be RAM-sized, got {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn a_state_from_another_cartridge_is_refused() {
        let mine = Cartridge::from_bytes(minimal_rom()).unwrap();
        let mut other_rom = minimal_rom();
        other_rom[0xA0..0xAC].copy_from_slice(b"SOMEONEELSE ");
        let theirs = Cartridge::from_bytes(other_rom).unwrap();

        let bytes = Gba::new(theirs).snapshot().to_bytes();
        assert!(matches!(
            Snapshot::from_bytes(&bytes, &mine),
            Err(GbaError::StateCartridge)
        ));
    }

    #[test]
    fn something_that_is_not_a_state_is_refused() {
        let cartridge = Cartridge::from_bytes(minimal_rom()).unwrap();
        assert!(matches!(
            Snapshot::from_bytes(b"", &cartridge),
            Err(GbaError::StateFormat)
        ));
        assert!(matches!(
            Snapshot::from_bytes(&[0x7F; 64], &cartridge),
            Err(GbaError::StateFormat)
        ));
    }

    /// A state from a build whose layout differs must say so rather than
    /// decode into a machine that only looks plausible.
    #[test]
    fn a_state_from_another_format_version_is_refused() {
        let cartridge = Cartridge::from_bytes(minimal_rom()).unwrap();
        let mut bytes = Gba::new(cartridge.clone()).snapshot().to_bytes();
        bytes[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(matches!(
            Snapshot::from_bytes(&bytes, &cartridge),
            Err(GbaError::StateVersion { found: 0xFFFF, .. })
        ));
    }

    #[test]
    fn a_truncated_state_is_refused_rather_than_decoded() {
        let cartridge = Cartridge::from_bytes(minimal_rom()).unwrap();
        let bytes = Gba::new(cartridge.clone()).snapshot().to_bytes();
        let cut = &bytes[..bytes.len() / 2];
        assert!(matches!(
            Snapshot::from_bytes(cut, &cartridge),
            Err(GbaError::StateCorrupt(_))
        ));
    }

    /// Audio in flight belongs to the moment the snapshot replaced.
    #[test]
    fn restoring_drops_buffered_audio() {
        let mut gba = booted();
        let snapshot = gba.snapshot();
        gba.run_frame();
        assert!(!gba.audio().is_empty(), "a frame should produce samples");
        gba.restore(&snapshot);
        assert!(gba.audio().is_empty());
    }
}
