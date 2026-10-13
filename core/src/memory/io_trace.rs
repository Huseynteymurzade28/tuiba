//! A log of writes to chosen I/O registers, for debugging.
//!
//! The bus records each write to a watched register as it happens; the
//! machine then stamps the new records with who wrote them and when,
//! since only it knows the instruction and the beam position. Off by
//! default: an untraced bus pays one `Option` check per I/O write.

use std::ops::Range;

use crate::memory::IO_SIZE;

/// What made a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// The instruction at this address (a BIOS call it made counts).
    Cpu(u32),
    /// A DMA channel.
    Dma(u8),
}

/// One write to a watched I/O register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoWrite {
    /// Offset into the I/O area (`0x0400_0000 + offset`).
    pub offset: u32,
    /// Bytes written: 1, 2 or 4.
    pub width: u8,
    /// The value as stored, before any register masks it.
    pub value: u32,
    /// Who wrote it.
    pub writer: Writer,
    /// Scanline (`VCOUNT`) when the write's instruction or DMA unit began.
    pub line: u16,
    /// Cycle within that scanline, 0 to 1231.
    pub cycle: u32,
}

/// The watch list and the writes not yet collected.
#[derive(Debug, Clone)]
pub struct IoTrace {
    watched: Box<[bool]>,
    writes: Vec<IoWrite>,
    /// Records before this index have been stamped.
    stamped: usize,
}

impl IoTrace {
    /// Watches every byte offset in `ranges` (clamped to the I/O area).
    #[must_use]
    pub fn new(ranges: impl IntoIterator<Item = Range<u32>>) -> Self {
        let mut watched = vec![false; IO_SIZE].into_boxed_slice();
        for range in ranges {
            let end = (range.end as usize).min(IO_SIZE);
            let start = (range.start as usize).min(end);
            watched[start..end].fill(true);
        }
        Self {
            watched,
            writes: Vec::new(),
            stamped: 0,
        }
    }

    /// Records a write of `width` bytes at `offset` if any of them is
    /// watched. It stays unstamped until [`IoTrace::stamp`].
    pub(crate) fn record(&mut self, offset: u32, width: u32, value: u32) {
        let start = offset as usize;
        let end = (start + width as usize).min(self.watched.len());
        if start < end && self.watched[start..end].contains(&true) {
            self.writes.push(IoWrite {
                offset,
                width: width as u8,
                value,
                writer: Writer::Cpu(0),
                line: 0,
                cycle: 0,
            });
        }
    }

    /// Whether writes are waiting for [`IoTrace::stamp`].
    #[must_use]
    pub(crate) fn has_unstamped(&self) -> bool {
        self.stamped < self.writes.len()
    }

    /// Attributes every write since the last stamp to `writer` at the
    /// given beam position.
    pub(crate) fn stamp(&mut self, writer: Writer, line: u16, cycle: u32) {
        for write in &mut self.writes[self.stamped..] {
            write.writer = writer;
            write.line = line;
            write.cycle = cycle;
        }
        self.stamped = self.writes.len();
    }

    /// Hands over the stamped writes, oldest first.
    pub fn take(&mut self) -> Vec<IoWrite> {
        let rest = self.writes.split_off(self.stamped);
        self.stamped = 0;
        std::mem::replace(&mut self.writes, rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_watched_bytes_are_recorded() {
        let mut trace = IoTrace::new([0x000..0x002, 0x208..0x20A]);
        trace.record(0x004, 2, 0x1234); // DISPSTAT: not watched
        trace.record(0x000, 2, 0x0403);
        trace.record(0x206, 4, 0x0001_0000); // WAITCNT+2..IME+2 overlaps IME
        trace.record(0x3FF, 4, 0); // runs off the end, not watched
        assert_eq!(trace.writes.len(), 2);
        assert_eq!(trace.writes[1].offset, 0x206);
    }

    #[test]
    fn stamping_and_taking() {
        let mut trace = IoTrace::new(Some(0..IO_SIZE as u32));
        trace.record(0x000, 2, 1);
        trace.stamp(Writer::Cpu(0x0800_0100), 10, 20);
        trace.record(0x0A0, 4, 2);
        assert!(trace.has_unstamped());
        let taken = trace.take();
        assert_eq!(taken.len(), 1, "unstamped writes stay");
        assert_eq!(taken[0].writer, Writer::Cpu(0x0800_0100));
        assert_eq!((taken[0].line, taken[0].cycle), (10, 20));
        trace.stamp(Writer::Dma(1), 11, 5);
        assert!(!trace.has_unstamped());
        let taken = trace.take();
        assert_eq!(taken[0].writer, Writer::Dma(1));
        assert_eq!(trace.take(), []);
    }
}
