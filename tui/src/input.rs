//! GBA keypad state from terminal key events.
//!
//! Which key means which button is decided in [`crate::bindings`]; this
//! module tracks *held* state. The GBA reports its ten buttons through
//! `KEYINPUT`, active-low. The hard part in a terminal is *releases*:
//! classic terminals only send key presses (plus auto-repeat). We handle
//! both worlds:
//!
//! - With the Kitty keyboard protocol (`REPORT_EVENT_TYPES`) we receive
//!   real `Release` events and track state exactly.
//! - Without it, a key is considered held for [`HOLD_TIMEOUT`] after the
//!   most recent press/repeat event, which auto-repeat keeps refreshing.

use std::time::{Duration, Instant};

use crossterm::event::KeyEventKind;
use tuiba_core::memory::io::KEYINPUT_ALL_RELEASED;

/// How long a key stays "held" after its last event when the terminal
/// cannot report releases. Long enough to bridge auto-repeat gaps (typical
/// initial delay 250–500 ms), short enough that taps feel responsive.
pub const HOLD_TIMEOUT: Duration = Duration::from_millis(500);

/// GBA buttons, numbered by their `KEYINPUT` bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum GbaKey {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Right = 4,
    Left = 5,
    Up = 6,
    Down = 7,
    R = 8,
    L = 9,
}

impl GbaKey {
    /// All buttons, in bit order.
    pub const ALL: [Self; 10] = [
        Self::A,
        Self::B,
        Self::Select,
        Self::Start,
        Self::Right,
        Self::Left,
        Self::Up,
        Self::Down,
        Self::R,
        Self::L,
    ];

    /// The `KEYINPUT` bit for this button.
    #[must_use]
    pub const fn mask(self) -> u16 {
        1 << (self as u16)
    }
}

/// One key's held state: exact with release events, by timeout without.
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    /// Time of the last press/repeat, `None` when released.
    last: Option<Instant>,
    /// Whether the terminal delivers `Release` events (Kitty protocol).
    release_events: bool,
}

impl Hold {
    /// A key that is not held.
    #[must_use]
    pub const fn new(release_events: bool) -> Self {
        Self {
            last: None,
            release_events,
        }
    }

    /// Feeds an event for this key.
    pub fn update(&mut self, kind: KeyEventKind, now: Instant) {
        self.last = match kind {
            KeyEventKind::Press | KeyEventKind::Repeat => Some(now),
            KeyEventKind::Release => None,
        };
    }

    /// Whether the key is held at `now`.
    #[must_use]
    pub fn is_held(&self, now: Instant) -> bool {
        match self.last {
            None => false,
            Some(_) if self.release_events => true,
            Some(at) => now.duration_since(at) < HOLD_TIMEOUT,
        }
    }
}

/// Tracks which GBA buttons are currently held.
#[derive(Debug)]
pub struct Keypad {
    holds: [Hold; 10],
    /// Whether the terminal delivers `Release` events (Kitty protocol).
    release_events: bool,
}

impl Keypad {
    /// A keypad with nothing pressed.
    ///
    /// `release_events` should come from
    /// `crossterm::terminal::supports_keyboard_enhancement()`.
    #[must_use]
    pub const fn new(release_events: bool) -> Self {
        Self {
            holds: [Hold::new(release_events); 10],
            release_events,
        }
    }

    /// Whether releases are tracked exactly rather than by timeout.
    #[must_use]
    pub const fn has_release_events(&self) -> bool {
        self.release_events
    }

    /// Feeds a key event that the bindings resolved to `button`.
    pub fn handle(&mut self, button: GbaKey, kind: KeyEventKind, now: Instant) {
        self.holds[button as usize].update(kind, now);
    }

    /// Whether `button` is held at `now`.
    #[must_use]
    pub fn is_pressed(&self, button: GbaKey, now: Instant) -> bool {
        self.holds[button as usize].is_held(now)
    }

    /// The `KEYINPUT` register value at `now` (active-low).
    #[must_use]
    pub fn keyinput(&self, now: Instant) -> u16 {
        GbaKey::ALL
            .iter()
            .fold(KEYINPUT_ALL_RELEASED, |bits, &button| {
                if self.is_pressed(button, now) {
                    bits & !button.mask()
                } else {
                    bits
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_tracking_with_release_events() {
        let t0 = Instant::now();
        let mut pad = Keypad::new(true);
        assert_eq!(pad.keyinput(t0), 0x03FF);
        pad.handle(GbaKey::A, KeyEventKind::Press, t0);
        pad.handle(GbaKey::Up, KeyEventKind::Press, t0);
        assert_eq!(
            pad.keyinput(t0),
            0x03FF & !(GbaKey::A.mask() | GbaKey::Up.mask())
        );
        // Still held long after: no timeout in this mode.
        let later = t0 + Duration::from_secs(10);
        assert!(pad.is_pressed(GbaKey::A, later));
        pad.handle(GbaKey::A, KeyEventKind::Release, later);
        assert_eq!(pad.keyinput(later), 0x03FF & !GbaKey::Up.mask());
    }

    #[test]
    fn timeout_release_without_release_events() {
        let t0 = Instant::now();
        let mut pad = Keypad::new(false);
        pad.handle(GbaKey::Start, KeyEventKind::Press, t0);
        assert!(pad.is_pressed(GbaKey::Start, t0 + HOLD_TIMEOUT / 2));
        assert!(!pad.is_pressed(GbaKey::Start, t0 + HOLD_TIMEOUT));
        // Auto-repeat refreshes the hold.
        pad.handle(
            GbaKey::Start,
            KeyEventKind::Repeat,
            t0 + Duration::from_millis(499),
        );
        assert!(pad.is_pressed(
            GbaKey::Start,
            t0 + HOLD_TIMEOUT + Duration::from_millis(100)
        ));
    }

    #[test]
    fn hold_ignores_release_timing_when_exact() {
        let t0 = Instant::now();
        let mut hold = Hold::new(true);
        assert!(!hold.is_held(t0));
        hold.update(KeyEventKind::Press, t0);
        assert!(hold.is_held(t0 + Duration::from_secs(60)));
        hold.update(KeyEventKind::Release, t0);
        assert!(!hold.is_held(t0));
    }
}
