//! Rewind: the last stretch of play, kept so it can be played backwards.
//!
//! Every other frame drawn, the machine is serialized ([`Snapshot::to_bytes`])
//! and the state before it is stored as a delta against it, so the
//! buffer holds one full state — the newest — and a chain of small
//! backward edits behind it. Stepping back decodes the newest delta
//! against the newest state and makes the result the new newest; the
//! oldest deltas fall off the front when the buffer outgrows its
//! budget. A minute of play costs tens of MiB rather than the gigabytes
//! whole states would.

use std::collections::VecDeque;

use tuiba_core::{Cartridge, Gba, Snapshot};

/// Frames between captures. Rewinding restores one capture per frame
/// drawn, so play runs backwards at this multiple of normal speed.
pub const INTERVAL: u32 = 2;

/// Memory the deltas may take before the oldest are dropped. Around a
/// minute of play in the homebrew tested; quieter games keep more.
const BUDGET: usize = 64 << 20;

/// The recent past of one game.
#[derive(Debug, Default)]
pub struct Rewind {
    /// The newest capture, whole.
    newest: Option<Vec<u8>>,
    /// Each entry turns the state after it (or `newest`, for the last)
    /// into the one before; oldest first.
    deltas: VecDeque<Vec<u8>>,
    /// Sum of the deltas' lengths.
    bytes: usize,
    /// Frames since the last capture.
    since_capture: u32,
}

impl Rewind {
    /// Notes that a frame was drawn, capturing the machine every
    /// [`INTERVAL`] frames. The frontend calls it once per frame shown,
    /// however many were emulated behind it (fast-forward), so a rewind
    /// retraces what the player saw.
    pub fn frame(&mut self, gba: &Gba) {
        self.since_capture += 1;
        if self.since_capture < INTERVAL && self.newest.is_some() {
            return;
        }
        self.since_capture = 0;
        let state = gba.snapshot().to_bytes();
        if let Some(previous) = self.newest.replace(state) {
            let delta = encode(self.newest.as_deref().unwrap_or_default(), &previous);
            self.bytes += delta.len();
            self.deltas.push_back(delta);
        }
        while self.bytes > BUDGET {
            let Some(oldest) = self.deltas.pop_front() else {
                break;
            };
            self.bytes -= oldest.len();
        }
    }

    /// Steps one capture back and returns the state to restore, or
    /// `None` when there is nothing older.
    pub fn step_back(&mut self, cartridge: &Cartridge) -> Option<Snapshot> {
        let newest = self.newest.as_deref()?;
        let delta = self.deltas.pop_back()?;
        self.bytes -= delta.len();
        let Some(older) = decode(newest, &delta) else {
            // Deltas are only ever made here, against this very state;
            // one that does not fit is a bug, and what is left of the
            // chain cannot be trusted either.
            self.clear();
            return None;
        };
        let snapshot = Snapshot::from_bytes(&older, cartridge).ok();
        self.newest = Some(older);
        // Play resumes from here; the next capture is a full interval on.
        self.since_capture = 0;
        snapshot
    }

    /// Whether there is anything to step back to.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Longest distance the encoder looks either way for bytes that moved.
const MAX_SHIFT: usize = 16;

/// Bytes that must match after a shift before the encoder believes it.
const RESYNC_WINDOW: usize = 16;

/// Matches shorter than this are cheaper to send as literals.
const MIN_COPY: usize = 4;

/// Delta operations, in the low two bits of each op's number: copy
/// that many bytes from the base cursor, take that many literal bytes
/// (moving the cursor past the bytes they replace), or move the cursor
/// by a zigzag-encoded distance.
const COPY: usize = 0;
const LITERAL: usize = 1;
const SEEK: usize = 2;

/// Encodes `target` as edits of `base`.
///
/// Consecutive states serialize to nearly the same bytes, but not at
/// the same offsets: postcard writes integers as varints, so a counter
/// that gains a byte shifts everything after it. The delta therefore
/// copies runs from `base` through a cursor that can be moved (`SEEK`)
/// when the bytes are found a little further along or back, and falls
/// back to literals where nothing matches.
fn encode(base: &[u8], target: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut literal_from = None;
    let flush = |out: &mut Vec<u8>, from: &mut Option<usize>, to: usize| {
        if let Some(start) = from.take() {
            put(out, ((to - start) << 2) | LITERAL);
            out.extend_from_slice(&target[start..to]);
        }
    };
    while i < target.len() {
        let run = common(&target[i..], base.get(j..).unwrap_or_default());
        if run >= MIN_COPY || (run > 0 && i + run == target.len()) {
            flush(&mut out, &mut literal_from, i);
            put(&mut out, (run << 2) | COPY);
            i += run;
            j += run;
            continue;
        }
        if let Some(shift) = resync(base, target, i, j) {
            flush(&mut out, &mut literal_from, i);
            let zigzag = if shift < 0 {
                (shift.unsigned_abs() << 1) - 1
            } else {
                (shift as usize) << 1
            };
            put(&mut out, (zigzag << 2) | SEEK);
            j = j.wrapping_add_signed(shift);
            continue;
        }
        literal_from.get_or_insert(i);
        i += 1;
        j += 1;
    }
    flush(&mut out, &mut literal_from, i);
    out
}

/// Rebuilds the target [`encode`] was given from the same `base`.
/// `None` if the delta does not fit the base.
fn decode(base: &[u8], delta: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(base.len());
    let (mut d, mut j) = (delta, 0usize);
    while !d.is_empty() {
        let op = take(&mut d)?;
        let n = op >> 2;
        match op & 3 {
            COPY => {
                out.extend_from_slice(base.get(j..j.checked_add(n)?)?);
                j += n;
            }
            LITERAL => {
                let (bytes, rest) = d.split_at_checked(n)?;
                out.extend_from_slice(bytes);
                d = rest;
                j += n;
            }
            SEEK => {
                j = if n & 1 == 1 {
                    j.checked_sub((n + 1) >> 1)?
                } else {
                    j.checked_add(n >> 1)?
                };
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Where `target[i..]` shows up in `base` near `j`, as an offset from `j`.
fn resync(base: &[u8], target: &[u8], i: usize, j: usize) -> Option<isize> {
    let want = target.get(i..i + RESYNC_WINDOW)?;
    (1..=MAX_SHIFT as isize).flat_map(|s| [s, -s]).find(|&s| {
        j.checked_add_signed(s)
            .and_then(|at| base.get(at..at + RESYNC_WINDOW))
            .is_some_and(|there| there == want)
    })
}

/// Length of the common prefix.
fn common(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Appends `n` as a LEB128 varint.
fn put(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

/// Reads a varint [`put`] wrote.
fn take(d: &mut &[u8]) -> Option<usize> {
    let mut n = 0usize;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = d.split_first()?;
        *d = rest;
        n |= usize::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(base: &[u8], target: &[u8]) -> usize {
        let delta = encode(base, target);
        assert_eq!(decode(base, &delta).as_deref(), Some(target));
        delta.len()
    }

    /// Something shaped like a serialized state: long runs, some noise.
    fn state(seed: u32, len: usize) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|i| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                if i % 7 == 0 {
                    (x >> 16) as u8
                } else {
                    (i / 64) as u8
                }
            })
            .collect()
    }

    #[test]
    fn identical_states_cost_almost_nothing() {
        let s = state(1, 100_000);
        assert!(round_trip(&s, &s) < 8);
    }

    #[test]
    fn shifted_bytes_are_found_again() {
        // A varint that grew by a byte near the front moves everything
        // after it; the delta must not turn into the whole state.
        let base = state(2, 100_000);
        let mut target = base.clone();
        target.insert(10, 0xAA);
        target.remove(50_000);
        target.insert(70_000, 1);
        target.insert(70_000, 2);
        assert!(round_trip(&base, &target) < 64);
    }

    #[test]
    fn anything_round_trips() {
        let a = state(3, 5_000);
        let b = state(4, 7_000);
        round_trip(&a, &b);
        round_trip(&b, &a);
        round_trip(&[], &a);
        round_trip(&a, &[]);
        round_trip(&a[..3], &a[..5]);
    }

    #[test]
    fn a_delta_for_another_base_is_refused() {
        let a = state(5, 1_000);
        let delta = encode(&a, &a);
        assert_eq!(decode(&a[..10], &delta), None);
    }

    fn cartridge() -> Cartridge {
        // An entry branch to itself: a machine with state that steps.
        let mut rom = vec![0u8; 0x200];
        rom[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
        Cartridge::from_bytes(rom).expect("valid ROM")
    }

    #[test]
    fn steps_back_through_the_captures_in_order() {
        let mut gba = Gba::new(cartridge());
        let mut rewind = Rewind::default();
        let mut seen = Vec::new();
        for frame in 0..10u16 {
            // Something that differs per frame and survives in the
            // machine: the keypad register.
            gba.set_keyinput(frame);
            gba.run_frame();
            rewind.frame(&gba);
            if frame % INTERVAL as u16 == 0 {
                seen.push(frame);
            }
        }
        let cart = gba.bus.cartridge.clone();
        // The newest capture is where play is; stepping back goes to the
        // ones before it, newest first.
        for &frame in seen.iter().rev().skip(1) {
            let snapshot = rewind.step_back(&cart).expect("a capture");
            gba.restore(&snapshot);
            assert_eq!(
                gba.bus.io.read16(tuiba_core::memory::io::reg::KEYINPUT),
                frame
            );
        }
        assert!(rewind.is_empty());
        assert!(rewind.step_back(&cart).is_none());
    }
}
