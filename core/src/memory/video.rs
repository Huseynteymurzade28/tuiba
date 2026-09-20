//! Video memory: palette RAM, VRAM and OAM.
//!
//! Kept separate from the bus so the PPU can borrow it read-only during
//! rendering without touching the rest of the system.

use crate::memory::{OAM_SIZE, PALETTE_SIZE, VRAM_SIZE};

/// The three video memories, each in its own mirror domain.
#[derive(Debug, Clone)]
pub struct VideoMemory {
    /// Palette RAM, 1 KiB, mirrored every 1 KiB.
    pub palette: Box<[u8]>,
    /// VRAM, 96 KiB. Mirrored every 128 KiB, with the upper 32 KiB of each
    /// 128 KiB block mirroring the block's `0x10000..0x18000` range.
    pub vram: Box<[u8]>,
    /// Object attribute memory, 1 KiB, mirrored every 1 KiB.
    pub oam: Box<[u8]>,
}

impl Default for VideoMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoMemory {
    /// Allocates zeroed video memory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            palette: vec![0; PALETTE_SIZE].into_boxed_slice(),
            vram: vec![0; VRAM_SIZE].into_boxed_slice(),
            oam: vec![0; OAM_SIZE].into_boxed_slice(),
        }
    }

    /// Maps a VRAM offset (low 24 bits of the address) into `0..VRAM_SIZE`.
    #[inline]
    #[must_use]
    pub const fn vram_index(offset: u32) -> usize {
        let off = (offset & 0x1_FFFF) as usize;
        if off >= VRAM_SIZE { off - 0x8000 } else { off }
    }

    /// Maps a palette offset into `0..PALETTE_SIZE`.
    #[inline]
    #[must_use]
    pub const fn palette_index(offset: u32) -> usize {
        (offset as usize) & (PALETTE_SIZE - 1)
    }

    /// Maps an OAM offset into `0..OAM_SIZE`.
    #[inline]
    #[must_use]
    pub const fn oam_index(offset: u32) -> usize {
        (offset as usize) & (OAM_SIZE - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vram_mirrors_upper_32k() {
        assert_eq!(VideoMemory::vram_index(0x0_0000), 0x0_0000);
        assert_eq!(VideoMemory::vram_index(0x1_7FFF), 0x1_7FFF);
        assert_eq!(VideoMemory::vram_index(0x1_8000), 0x1_0000);
        assert_eq!(VideoMemory::vram_index(0x1_FFFF), 0x1_7FFF);
        assert_eq!(VideoMemory::vram_index(0x2_0000), 0x0_0000);
        assert_eq!(VideoMemory::vram_index(0xFF_FFFF), 0x1_7FFF);
    }

    #[test]
    fn small_regions_wrap() {
        assert_eq!(VideoMemory::palette_index(0x400), 0);
        assert_eq!(VideoMemory::oam_index(0x7FF), 0x3FF);
    }
}
