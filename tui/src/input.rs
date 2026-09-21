//! Keyboard → GBA keypad mapping.
//!
//! The GBA reports its ten buttons through `KEYINPUT`, active-low. The
//! hard part in a terminal is *releases*: classic terminals only send key
//! presses (plus auto-repeat). We handle both worlds:
//!
//! - With the Kitty keyboard protocol (`REPORT_EVENT_TYPES`) we receive
//!   real `Release` events and track state exactly.
//! - Without it, a key is considered held for [`HOLD_TIMEOUT`] after the
//!   most recent press/repeat event, which auto-repeat keeps refreshing.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

/// Maps a terminal key to a GBA button.
///
/// Buttons are on the keys of the same name: `A`, `B`, `L`, `R`,
/// arrows for the D-pad, `Enter` = Start, `Space` / `Backspace` = Select.
/// `Z`/`X` double as A/B for people used to other emulators.
#[must_use]
pub fn map_key(code: KeyCode) -> Option<GbaKey> {
    Some(match code {
        KeyCode::Up => GbaKey::Up,
        KeyCode::Down => GbaKey::Down,
        KeyCode::Left => GbaKey::Left,
        KeyCode::Right => GbaKey::Right,
        KeyCode::Char('a' | 'A' | 'z' | 'Z') => GbaKey::A,
        KeyCode::Char('b' | 'B' | 'x' | 'X') => GbaKey::B,
        KeyCode::Char('l' | 'L') => GbaKey::L,
        KeyCode::Char('r' | 'R') => GbaKey::R,
        KeyCode::Enter => GbaKey::Start,
        KeyCode::Backspace | KeyCode::Char(' ') => GbaKey::Select,
        _ => return None,
    })
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

    /// Feeds a terminal key event. Returns `true` if it mapped to a button.
    pub fn handle(&mut self, key: KeyEvent, now: Instant) -> bool {
        // Shift as Select: crossterm reports the modifier alone only with
        // the enhanced protocol, so this is a bonus, not the only binding.
        let mapped = match key.code {
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::RightShift) => {
                Some(GbaKey::Select)
            }
            code => map_key(code),
        };
        let Some(button) = mapped else { return false };
        // Ignore chords like Ctrl+Z so terminal shortcuts stay usable.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        self.holds[button as usize].update(key.kind, now);
        true
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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Release)
    }

    #[test]
    fn mapping_covers_all_ten_buttons() {
        let codes = [
            KeyCode::Char('a'),
            KeyCode::Char('b'),
            KeyCode::Backspace,
            KeyCode::Enter,
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char('r'),
            KeyCode::Char('l'),
        ];
        for (code, expected) in codes.into_iter().zip(GbaKey::ALL) {
            assert_eq!(map_key(code), Some(expected));
        }
        assert_eq!(map_key(KeyCode::Char('q')), None);
        assert_eq!(
            map_key(KeyCode::Char('Z')),
            Some(GbaKey::A),
            "shifted letters still map"
        );
    }

    #[test]
    fn exact_tracking_with_release_events() {
        let t0 = Instant::now();
        let mut pad = Keypad::new(true);
        assert_eq!(pad.keyinput(t0), 0x03FF);
        assert!(pad.handle(press(KeyCode::Char('a')), t0));
        assert!(pad.handle(press(KeyCode::Up), t0));
        assert_eq!(
            pad.keyinput(t0),
            0x03FF & !(GbaKey::A.mask() | GbaKey::Up.mask())
        );
        // Still held long after: no timeout in this mode.
        assert!(pad.is_pressed(GbaKey::A, t0 + Duration::from_secs(10)));
        pad.handle(release(KeyCode::Char('a')), t0 + Duration::from_secs(10));
        assert_eq!(
            pad.keyinput(t0 + Duration::from_secs(10)),
            0x03FF & !GbaKey::Up.mask()
        );
    }

    #[test]
    fn timeout_release_without_release_events() {
        let t0 = Instant::now();
        let mut pad = Keypad::new(false);
        pad.handle(press(KeyCode::Enter), t0);
        assert!(pad.is_pressed(GbaKey::Start, t0 + HOLD_TIMEOUT / 2));
        assert!(!pad.is_pressed(GbaKey::Start, t0 + HOLD_TIMEOUT));
        // Auto-repeat refreshes the hold.
        let repeat =
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat);
        pad.handle(repeat, t0 + Duration::from_millis(499));
        assert!(pad.is_pressed(
            GbaKey::Start,
            t0 + HOLD_TIMEOUT + Duration::from_millis(100)
        ));
    }

    #[test]
    fn modifier_chords_are_ignored() {
        let t0 = Instant::now();
        let mut pad = Keypad::new(true);
        assert!(!pad.handle(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), t0));
        assert!(!pad.handle(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT), t0));
        assert_eq!(pad.keyinput(t0), 0x03FF);
        assert!(pad.handle(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT), t0));
    }

    #[test]
    fn unmapped_keys_are_reported_unhandled() {
        let mut pad = Keypad::new(true);
        assert!(!pad.handle(press(KeyCode::Char('q')), Instant::now()));
    }
}
