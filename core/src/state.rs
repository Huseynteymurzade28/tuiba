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

use crate::gba::Gba;
use crate::ppu::Framebuffer;

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
    use crate::Memory;
    use crate::cpu::registers::PC;
    use crate::memory::cartridge::HEADER_END;

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
