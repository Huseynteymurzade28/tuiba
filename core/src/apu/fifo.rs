//! The two "direct sound" FIFOs: 32-byte queues of signed 8-bit samples
//! that a timer overflow advances and a DMA channel keeps topped up.

/// Bytes the FIFO holds.
pub const CAPACITY: usize = 32;
/// A FIFO at or below this fill level asks its DMA channel for more data
/// (one transfer brings 16 bytes).
const REFILL_THRESHOLD: usize = 16;

/// One direct-sound FIFO.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Fifo {
    buffer: [u8; CAPACITY],
    read: usize,
    len: usize,
    /// The sample most recently taken out, which is what the mixer hears
    /// until the next timer overflow.
    sample: i8,
}

impl Default for Fifo {
    fn default() -> Self {
        Self::new()
    }
}

impl Fifo {
    /// An empty FIFO.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0; CAPACITY],
            read: 0,
            len: 0,
            sample: 0,
        }
    }

    /// Queues `bytes` in order. Data that does not fit is dropped.
    pub fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.len == CAPACITY {
                return;
            }
            self.buffer[(self.read + self.len) % CAPACITY] = byte;
            self.len += 1;
        }
    }

    /// Empties the queue and silences the output (`SOUNDCNT_H` reset bit).
    pub fn reset(&mut self) {
        self.read = 0;
        self.len = 0;
        self.sample = 0;
    }

    /// Takes the next sample, on a timer overflow. An empty FIFO keeps
    /// repeating its last sample. Returns `true` when the queue has run
    /// low enough for a DMA refill.
    pub fn pop(&mut self) -> bool {
        if self.len > 0 {
            self.sample = self.buffer[self.read] as i8;
            self.read = (self.read + 1) % CAPACITY;
            self.len -= 1;
        }
        self.len <= REFILL_THRESHOLD
    }

    /// The current output sample.
    #[must_use]
    pub fn sample(&self) -> i8 {
        self.sample
    }

    /// Bytes queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_come_out_in_order_and_the_last_one_sticks() {
        let mut fifo = Fifo::new();
        fifo.push(&[1, 2, 0xFF]);
        assert_eq!(fifo.sample(), 0);
        assert!(fifo.pop());
        assert_eq!(fifo.sample(), 1);
        fifo.pop();
        fifo.pop();
        assert_eq!(fifo.sample(), -1);
        assert!(fifo.is_empty());
        fifo.pop();
        assert_eq!(fifo.sample(), -1, "repeats when starved");
    }

    #[test]
    fn refill_is_requested_at_half_full() {
        let mut fifo = Fifo::new();
        fifo.push(&[0; 32]);
        assert_eq!(fifo.len(), 32);
        for expected in 0..15 {
            assert!(!fifo.pop(), "pop {expected}: still above threshold");
        }
        assert!(fifo.pop(), "16 left");
        fifo.push(&[0; 16]);
        assert_eq!(fifo.len(), 32);
        fifo.push(&[0; 4]);
        assert_eq!(fifo.len(), 32, "overflow dropped");
    }

    #[test]
    fn wraps_around_the_ring() {
        let mut fifo = Fifo::new();
        fifo.push(&[0; 30]);
        for _ in 0..30 {
            fifo.pop();
        }
        fifo.push(&[7, 8, 9, 10]);
        fifo.pop();
        fifo.pop();
        fifo.pop();
        fifo.pop();
        assert_eq!(fifo.sample(), 10);
        fifo.reset();
        assert_eq!(fifo.sample(), 0);
        assert!(fifo.is_empty());
    }
}
