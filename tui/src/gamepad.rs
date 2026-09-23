//! Game controllers, read straight from the OS.
//!
//! The terminal only ever sees the keyboard, so pads are polled through
//! gilrs (evdev on Linux, Windows Gaming Input, `IOKit` on macOS) once per
//! frame, next to the keypad rather than through the event loop. What
//! the frontend gets is a [`PadSet`]: the buttons held on any connected
//! pad right now, with the left stick folded into the D-pad. Presses are
//! the difference between two of them, so a stick pushed past the
//! threshold is a press like any other.
//!
//! Buttons are named by position, not by label: `east` is the right-hand
//! face button whether the pad prints A, B or a circle on it. That keeps
//! the default layout where a GBA player's thumb expects it — A on the
//! right, B below — on every brand.
//!
//! Built without the `gamepad` feature, [`Gamepads`] exists but never
//! sees a pad; the names still parse, so a keys file works in both builds.

// The stub never reads a pad, so the reading half of this module is idle.
#![cfg_attr(
    not(feature = "gamepad"),
    allow(dead_code, unused_mut, clippy::unused_self)
)]

/// A button on a game controller, by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    Mode,
    LeftStick,
    RightStick,
    Up,
    Down,
    Left,
    Right,
}

impl PadButton {
    /// Every button, in the order the file names are listed.
    pub const ALL: [Self; 17] = [
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::Select,
        Self::Start,
        Self::Mode,
        Self::LeftStick,
        Self::RightStick,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
    ];

    /// The name after `pad:` in the keys file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::North => "north",
            Self::L1 => "l1",
            Self::R1 => "r1",
            Self::L2 => "l2",
            Self::R2 => "r2",
            Self::Select => "select",
            Self::Start => "start",
            Self::Mode => "mode",
            Self::LeftStick => "lstick",
            Self::RightStick => "rstick",
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// The button called `name` (case-insensitive).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|b| b.name().eq_ignore_ascii_case(name))
    }

    const fn mask(self) -> u32 {
        1 << (self as u32)
    }
}

/// Whose labels a pad has printed on it, for showing its buttons the
/// way the player sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadStyle {
    /// A B X Y, LB RB LT RT, View and Menu.
    Xbox,
    /// Cross, circle, square, triangle; Share and Options.
    PlayStation,
    /// B A Y X — the face letters mirrored from Xbox — ZL ZR, − and +.
    Nintendo,
    /// Anything else: positions and generic shoulder names.
    Generic,
}

impl PadStyle {
    /// Guesses the style from the name the OS gives the pad.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
        if has(&["xbox", "x-box", "microsoft"]) {
            Self::Xbox
        } else if has(&[
            "playstation",
            "dualsense",
            "dualshock",
            "sony",
            "ps3",
            "ps4",
            "ps5",
        ]) {
            Self::PlayStation
        } else if has(&["nintendo", "pro controller", "joy-con", "switch"]) {
            Self::Nintendo
        } else {
            Self::Generic
        }
    }

    /// The button that confirms in a menu: the bottom one, except on
    /// Nintendo pads where it is the right one. (In a game the GBA's A
    /// stays on the right whatever the brand — that is about where the
    /// thumb goes, this is about habit.)
    #[must_use]
    pub const fn confirm(self) -> PadButton {
        match self {
            Self::Nintendo => PadButton::East,
            _ => PadButton::South,
        }
    }

    /// The button that backs out of a menu, opposite [`Self::confirm`].
    #[must_use]
    pub const fn back(self) -> PadButton {
        match self {
            Self::Nintendo => PadButton::South,
            _ => PadButton::East,
        }
    }

    /// What `button` is called on this kind of pad.
    #[must_use]
    pub const fn label(self, button: PadButton) -> &'static str {
        use PadButton as B;
        match (self, button) {
            (_, B::Up) => "↑",
            (_, B::Down) => "↓",
            (_, B::Left) => "←",
            (_, B::Right) => "→",
            (Self::Xbox, B::South) | (Self::Nintendo, B::East) => "A",
            (Self::Xbox, B::East) | (Self::Nintendo, B::South) => "B",
            (Self::Xbox, B::West) | (Self::Nintendo, B::North) => "X",
            (Self::Xbox, B::North) | (Self::Nintendo, B::West) => "Y",
            (Self::Xbox, B::L1) => "LB",
            (Self::Xbox, B::R1) => "RB",
            (Self::Xbox, B::L2) => "LT",
            (Self::Xbox, B::R2) => "RT",
            (Self::Xbox, B::Select) => "View",
            (Self::Xbox, B::Start) => "Menu",
            (Self::Xbox, B::Mode) => "Xbox",
            (Self::PlayStation, B::South) => "✕",
            (Self::PlayStation, B::East) => "○",
            (Self::PlayStation, B::West) => "□",
            (Self::PlayStation, B::North) => "△",
            (Self::PlayStation, B::Select) => "Share",
            (Self::PlayStation, B::Start) => "Options",
            (Self::PlayStation, B::Mode) => "PS",
            (Self::Nintendo, B::L1) => "L",
            (Self::Nintendo, B::R1) => "R",
            (Self::Nintendo, B::L2) => "ZL",
            (Self::Nintendo, B::R2) => "ZR",
            (Self::Nintendo, B::Select) => "−",
            (Self::Nintendo, B::Start) => "+",
            (Self::Nintendo, B::Mode) => "Home",
            (Self::Xbox | Self::Nintendo, B::LeftStick) => "LS",
            (Self::Xbox | Self::Nintendo, B::RightStick) => "RS",
            (_, B::L1) => "L1",
            (_, B::R1) => "R1",
            (_, B::L2) => "L2",
            (_, B::R2) => "R2",
            (_, B::LeftStick) => "L3",
            (_, B::RightStick) => "R3",
            (Self::Generic, B::South) => "south",
            (Self::Generic, B::East) => "east",
            (Self::Generic, B::West) => "west",
            (Self::Generic, B::North) => "north",
            (Self::Generic, B::Select) => "select",
            (Self::Generic, B::Start) => "start",
            (Self::Generic, B::Mode) => "mode",
        }
    }
}

/// A set of pad buttons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadSet(u32);

impl PadSet {
    /// No buttons.
    pub const EMPTY: Self = Self(0);

    /// Adds a button.
    pub const fn insert(&mut self, button: PadButton) {
        self.0 |= button.mask();
    }

    /// Whether `button` is in the set.
    #[must_use]
    pub const fn contains(self, button: PadButton) -> bool {
        self.0 & button.mask() != 0
    }

    /// Buttons in `self` that are not in `before`: what was just pressed.
    #[must_use]
    pub const fn since(self, before: Self) -> Self {
        Self(self.0 & !before.0)
    }

    /// Buttons in both sets.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The buttons in the set.
    pub fn iter(self) -> impl Iterator<Item = PadButton> {
        PadButton::ALL
            .into_iter()
            .filter(move |&b| self.contains(b))
    }
}

/// How far a stick has to lean before it counts as a D-pad direction.
/// Well past gilrs' own deadzone, so a resting stick that has drifted
/// does not walk the character.
const STICK_THRESHOLD: f32 = 0.5;

/// The D-pad directions a stick at (`x`, `y`) stands for; `y` grows
/// upwards. Diagonals give two directions, as on a D-pad.
#[must_use]
pub fn stick_directions(x: f32, y: f32) -> PadSet {
    let mut set = PadSet::EMPTY;
    for (lean, button) in [
        (y, PadButton::Up),
        (-y, PadButton::Down),
        (-x, PadButton::Left),
        (x, PadButton::Right),
    ] {
        if lean >= STICK_THRESHOLD {
            set.insert(button);
        }
    }
    set
}

/// Something the status bar should mention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadNotice {
    /// A pad was plugged in (or was already there at start-up).
    Connected(String),
    /// A pad went away.
    Disconnected(String),
}

/// Every connected pad, as one.
pub struct Gamepads {
    #[cfg(feature = "gamepad")]
    gilrs: Option<gilrs::Gilrs>,
}

impl Gamepads {
    /// Whether this build can read pads at all.
    pub const SUPPORTED: bool = cfg!(feature = "gamepad");

    /// Starts listening for pads. Never fails: without a backend (no
    /// udev, say) there are simply never any pads.
    #[must_use]
    pub fn open() -> Self {
        Self {
            #[cfg(feature = "gamepad")]
            gilrs: gilrs::Gilrs::new().ok(),
        }
    }

    /// Takes in everything the pads did since the last call, and says
    /// which pads came and went. Must run before [`Self::held`] for it to
    /// be current.
    pub fn poll(&mut self) -> Vec<PadNotice> {
        let mut notices = Vec::new();
        #[cfg(feature = "gamepad")]
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                let name = || gilrs.gamepad(event.id).name().to_string();
                match event.event {
                    gilrs::EventType::Connected => notices.push(PadNotice::Connected(name())),
                    gilrs::EventType::Disconnected => {
                        notices.push(PadNotice::Disconnected(name()));
                    }
                    _ => {}
                }
            }
        }
        notices
    }

    /// Buttons held right now on any connected pad, the left stick
    /// counting as the D-pad.
    #[must_use]
    pub fn held(&self) -> PadSet {
        let mut set = PadSet::EMPTY;
        #[cfg(feature = "gamepad")]
        if let Some(gilrs) = &self.gilrs {
            for (_, pad) in gilrs.gamepads() {
                for button in PadButton::ALL {
                    if pad.is_pressed(backend_button(button)) {
                        set.insert(button);
                    }
                }
                let stick = stick_directions(
                    pad.value(gilrs::Axis::LeftStickX),
                    pad.value(gilrs::Axis::LeftStickY),
                );
                set.0 |= stick.0;
            }
        }
        set
    }

    /// The labels of the first connected pad, or `None` without a pad.
    #[must_use]
    pub fn style(&self) -> Option<PadStyle> {
        #[cfg(feature = "gamepad")]
        if let Some(gilrs) = &self.gilrs {
            return gilrs
                .gamepads()
                .next()
                .map(|(_, pad)| PadStyle::from_name(pad.name()));
        }
        None
    }
}

/// The gilrs name for a position.
#[cfg(feature = "gamepad")]
const fn backend_button(button: PadButton) -> gilrs::Button {
    use gilrs::Button;
    match button {
        PadButton::South => Button::South,
        PadButton::East => Button::East,
        PadButton::West => Button::West,
        PadButton::North => Button::North,
        PadButton::L1 => Button::LeftTrigger,
        PadButton::R1 => Button::RightTrigger,
        PadButton::L2 => Button::LeftTrigger2,
        PadButton::R2 => Button::RightTrigger2,
        PadButton::Select => Button::Select,
        PadButton::Start => Button::Start,
        PadButton::Mode => Button::Mode,
        PadButton::LeftStick => Button::LeftThumb,
        PadButton::RightStick => Button::RightThumb,
        PadButton::Up => Button::DPadUp,
        PadButton::Down => Button::DPadDown,
        PadButton::Left => Button::DPadLeft,
        PadButton::Right => Button::DPadRight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for button in PadButton::ALL {
            assert_eq!(PadButton::from_name(button.name()), Some(button));
        }
        assert_eq!(PadButton::from_name("EAST"), Some(PadButton::East));
        assert_eq!(PadButton::from_name("a"), None, "labels are not positions");
    }

    #[test]
    fn labels_follow_the_brand() {
        assert_eq!(
            PadStyle::from_name("Xbox Series Controller"),
            PadStyle::Xbox
        );
        assert_eq!(
            PadStyle::from_name("DualSense Wireless Controller"),
            PadStyle::PlayStation
        );
        assert_eq!(
            PadStyle::from_name("Nintendo Switch Pro Controller"),
            PadStyle::Nintendo
        );
        assert_eq!(PadStyle::from_name("8BitDo SN30"), PadStyle::Generic);
        // The same position carries a different letter per brand.
        assert_eq!(PadStyle::Xbox.label(PadButton::East), "B");
        assert_eq!(PadStyle::Nintendo.label(PadButton::East), "A");
        assert_eq!(PadStyle::PlayStation.label(PadButton::East), "○");
        assert_eq!(PadStyle::Generic.label(PadButton::L2), "L2");
        // Menus confirm with whatever the brand prints its "yes" on.
        assert_eq!(PadStyle::Xbox.label(PadStyle::Xbox.confirm()), "A");
        assert_eq!(PadStyle::Nintendo.label(PadStyle::Nintendo.confirm()), "A");
        assert_eq!(
            PadStyle::PlayStation.label(PadStyle::PlayStation.back()),
            "○"
        );
    }

    #[test]
    fn stick_needs_a_real_lean() {
        assert_eq!(stick_directions(0.0, 0.0), PadSet::EMPTY);
        assert_eq!(stick_directions(0.3, -0.4), PadSet::EMPTY, "drift");
        let up: Vec<_> = stick_directions(0.1, 0.9).iter().collect();
        assert_eq!(up, [PadButton::Up]);
        let diagonal: Vec<_> = stick_directions(-0.7, -0.7).iter().collect();
        assert_eq!(diagonal, [PadButton::Down, PadButton::Left]);
    }

    #[test]
    fn presses_are_what_is_new() {
        let mut before = PadSet::EMPTY;
        before.insert(PadButton::East);
        let mut now = before;
        now.insert(PadButton::Start);
        let pressed: Vec<_> = now.since(before).iter().collect();
        assert_eq!(pressed, [PadButton::Start]);
        assert_eq!(before.since(now), PadSet::EMPTY, "a release is no press");
    }
}
