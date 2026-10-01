//! Video writes made while the PPU is drawing a line.
//!
//! The PPU draws a whole scanline at once, when the line enters HBlank.
//! On hardware the line is drawn dot by dot during the visible part, so
//! a write that lands halfway through — a late HBlank handler, a DMA
//! that runs into the next line — changes only the pixels after it.
//!
//! While a line is being drawn the bus records every byte it changes in
//! the registers, palette, VRAM and OAM, and the system marks the dot at
//! which each batch landed. When the line is drawn, the PPU winds the
//! changes back, draws up to the first mark, replays that batch, draws
//! on to the next mark, and so on. A line nobody writes to during its
//! visible part costs nothing extra.

use crate::memory::VideoMemory;
use crate::memory::io::IoRegisters;

/// Register offsets the renderer reads: everything up to `BLDY`.
pub const IO_RANGE: u32 = 0x60;

/// Dots are rounded down to this granularity, which caps the number of
/// partial draws per line (a DMA streaming into VRAM mid-line changes
/// something every few cycles).
const DOT_STEP: u16 = 8;

/// Where a journalled byte lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The I/O register block, below [`IO_RANGE`].
    Io,
    /// Palette RAM.
    Palette,
    /// VRAM.
    Vram,
    /// OAM.
    Oam,
}

/// One byte that changed.
#[derive(Debug, Clone, Copy)]
struct Change {
    target: Target,
    index: usize,
    old: u8,
    new: u8,
}

/// The changes made during the visible part of the current line.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    enabled: bool,
    changes: Vec<Change>,
    /// `(changes recorded so far, dot)`: the changes before the count
    /// landed at that dot.
    marks: Vec<(usize, u16)>,
}

impl Journal {
    /// Turns recording on while the PPU is drawing a line, off otherwise.
    pub fn enable(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether writes are being recorded.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Records the bytes at `index` going from `old` to `new`.
    pub fn record(&mut self, target: Target, index: usize, old: &[u8], new: &[u8]) {
        for (i, (&old, &new)) in old.iter().zip(new).enumerate() {
            if old != new {
                self.changes.push(Change {
                    target,
                    index: index + i,
                    old,
                    new,
                });
            }
        }
    }

    /// Stamps the changes recorded since the last mark with `dot`, the
    /// pixel of the current line at which they took effect.
    pub fn mark(&mut self, dot: u16) {
        let end = self.changes.len();
        if end > self.marks.last().map_or(0, |&(n, _)| n) {
            self.marks.push((end, dot - dot % DOT_STEP));
        }
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The marks, in the order they were made.
    pub(crate) fn marks(&self) -> &[(usize, u16)] {
        &self.marks
    }

    /// Number of recorded changes.
    pub(crate) fn len(&self) -> usize {
        self.changes.len()
    }

    /// Puts every recorded byte back the way it was before the line.
    pub(crate) fn undo_all(&self, io: &mut IoRegisters, video: &mut VideoMemory) {
        for change in self.changes.iter().rev() {
            put(io, video, change, change.old);
        }
    }

    /// Re-applies changes `range`, in order.
    pub(crate) fn redo(
        &self,
        range: std::ops::Range<usize>,
        io: &mut IoRegisters,
        video: &mut VideoMemory,
    ) {
        for change in &self.changes[range] {
            put(io, video, change, change.new);
        }
    }

    /// Forgets the line's changes.
    pub fn clear(&mut self) {
        self.changes.clear();
        self.marks.clear();
    }
}

fn put(io: &mut IoRegisters, video: &mut VideoMemory, change: &Change, value: u8) {
    match change.target {
        Target::Io => io.set_raw8(change.index as u32, value),
        Target::Palette => video.palette[change.index] = value,
        Target::Vram => video.vram[change.index] = value,
        Target::Oam => video.oam[change.index] = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_bytes_are_not_recorded_and_marks_skip_empty_batches() {
        let mut journal = Journal::default();
        journal.record(Target::Vram, 4, &[1, 2], &[1, 3]);
        assert_eq!(journal.len(), 1);
        journal.mark(37);
        journal.mark(50); // nothing new
        assert_eq!(journal.marks(), &[(1, 32)]);
    }

    #[test]
    fn undo_and_redo_restore_both_sides() {
        let (mut io, mut video) = (IoRegisters::new(), VideoMemory::new());
        let mut journal = Journal::default();
        video.palette[0] = 9;
        io.set_raw8(0x10, 7);
        journal.record(Target::Palette, 0, &[1], &[9]);
        journal.record(Target::Io, 0x10, &[2], &[7]);
        journal.undo_all(&mut io, &mut video);
        assert_eq!((video.palette[0], io.read8(0x10)), (1, 2));
        journal.redo(0..1, &mut io, &mut video);
        assert_eq!((video.palette[0], io.read8(0x10)), (9, 2));
    }
}
