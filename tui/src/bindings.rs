//! Bindings: which terminal key or pad button does what in a game.
//!
//! The defaults live in [`Bindings::defaults`]; a `keys` file in the
//! configuration directory ([`crate::library::config_dir`]) overrides them
//! one action per line:
//!
//! ```text
//! # button = key [key ...]
//! a      = a z pad:east
//! b      = b x
//! select = space backspace rshift
//! fast   = tab f pad:r2
//! step   = .
//! ```
//!
//! Actions not mentioned keep their defaults. A line replaces the
//! bindings of the kinds it names — keys, pad buttons (`pad:` + a name
//! from [`PadButton`]), or both — so `b = b x` above keeps B's default
//! pad button. An empty right-hand side unbinds everything. Letters match
//! regardless of case. `Esc`, `Ctrl+Q` and `?`
//! are fixed so there is always a way out and a way to see the bindings.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, ModifierKeyCode};

use crate::gamepad::PadButton;
use crate::input::GbaKey;
use crate::library::config_dir;

/// Something a key can do in a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Press a GBA button.
    Button(GbaKey),
    /// Toggle pause.
    Pause,
    /// Run one frame while paused.
    Step,
    /// Run uncapped while held.
    FastForward,
    /// Toggle sound output.
    Mute,
    /// Freeze the machine into the quick slot.
    SaveState,
    /// Put the quick slot back.
    LoadState,
    /// Open the save-state panel.
    States,
    /// Back to the library, pressed twice like `Esc`.
    Leave,
    /// Show the bindings, like `?`.
    Help,
}

impl Action {
    /// Every action, in the order the help overlay and the file use.
    pub const ALL: [Self; 19] = [
        Self::Button(GbaKey::Up),
        Self::Button(GbaKey::Down),
        Self::Button(GbaKey::Left),
        Self::Button(GbaKey::Right),
        Self::Button(GbaKey::A),
        Self::Button(GbaKey::B),
        Self::Button(GbaKey::L),
        Self::Button(GbaKey::R),
        Self::Button(GbaKey::Start),
        Self::Button(GbaKey::Select),
        Self::Pause,
        Self::Step,
        Self::FastForward,
        Self::Mute,
        Self::SaveState,
        Self::LoadState,
        Self::States,
        Self::Leave,
        Self::Help,
    ];

    /// The name used in the keys file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Button(GbaKey::A) => "a",
            Self::Button(GbaKey::B) => "b",
            Self::Button(GbaKey::Select) => "select",
            Self::Button(GbaKey::Start) => "start",
            Self::Button(GbaKey::Right) => "right",
            Self::Button(GbaKey::Left) => "left",
            Self::Button(GbaKey::Up) => "up",
            Self::Button(GbaKey::Down) => "down",
            Self::Button(GbaKey::R) => "r",
            Self::Button(GbaKey::L) => "l",
            Self::Pause => "pause",
            Self::Step => "step",
            Self::FastForward => "fast",
            Self::Mute => "mute",
            Self::SaveState => "save",
            Self::LoadState => "load",
            Self::States => "states",
            Self::Leave => "leave",
            Self::Help => "help",
        }
    }

    /// What the help overlay calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Button(GbaKey::A) => "A",
            Self::Button(GbaKey::B) => "B",
            Self::Button(GbaKey::Select) => "Select",
            Self::Button(GbaKey::Start) => "Start",
            Self::Button(GbaKey::Right) => "D-pad right",
            Self::Button(GbaKey::Left) => "D-pad left",
            Self::Button(GbaKey::Up) => "D-pad up",
            Self::Button(GbaKey::Down) => "D-pad down",
            Self::Button(GbaKey::R) => "R",
            Self::Button(GbaKey::L) => "L",
            Self::Pause => "pause",
            Self::Step => "one frame (paused)",
            Self::FastForward => "fast-forward (hold)",
            Self::Mute => "mute sound",
            Self::SaveState => "save state",
            Self::LoadState => "load state",
            Self::States => "save-state panel",
            Self::Leave => "library (twice)",
            Self::Help => "this list",
        }
    }

    /// The key that always does this and cannot be unbound, if any.
    #[must_use]
    pub const fn fixed_key(self) -> Option<&'static str> {
        match self {
            Self::Leave => Some("esc"),
            Self::Help => Some("?"),
            _ => None,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// Something that can be bound: a terminal key or a pad button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A key, as crossterm reports it.
    Key(KeyCode),
    /// A button on any connected pad.
    Pad(PadButton),
}

impl Input {
    /// Whether `self` and `other` are both keys or both pad buttons.
    const fn same_kind(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Key(_), Self::Key(_)) | (Self::Pad(_), Self::Pad(_))
        )
    }
}

/// The active input → action table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    /// Keys are stored normalised (letters lower-case); an input appears
    /// at most once.
    table: Vec<(Input, Action)>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Bindings {
    /// The built-in bindings: buttons on the keys of the same name,
    /// arrows for the D-pad, `Enter` = Start, `Space` / `Backspace` /
    /// right Shift = Select, `Z`/`X` doubling as A/B, `P` pause, `.` step,
    /// `Tab` / `F` fast-forward and `M` mute. On a pad, the buttons sit
    /// where a GBA has them — A east, B south, the shoulders on L1/R1 —
    /// R2 fast-forwards, L2 opens the save-state panel, north pauses,
    /// the guide button (twice) leaves and the right stick shows the
    /// bindings. Quick save and load stay off the pad: one stray press
    /// of a load throws progress away.
    #[must_use]
    pub fn defaults() -> Self {
        use KeyCode::Char;
        let mut b = Self { table: Vec::new() };
        let bind = |b: &mut Self, action: Action, keys: &[KeyCode]| {
            for &key in keys {
                b.bind(Input::Key(key), action);
            }
        };
        bind(&mut b, Action::Button(GbaKey::Up), &[KeyCode::Up]);
        bind(&mut b, Action::Button(GbaKey::Down), &[KeyCode::Down]);
        bind(&mut b, Action::Button(GbaKey::Left), &[KeyCode::Left]);
        bind(&mut b, Action::Button(GbaKey::Right), &[KeyCode::Right]);
        bind(&mut b, Action::Button(GbaKey::A), &[Char('a'), Char('z')]);
        bind(&mut b, Action::Button(GbaKey::B), &[Char('b'), Char('x')]);
        bind(&mut b, Action::Button(GbaKey::L), &[Char('l')]);
        bind(&mut b, Action::Button(GbaKey::R), &[Char('r')]);
        bind(&mut b, Action::Button(GbaKey::Start), &[KeyCode::Enter]);
        bind(
            &mut b,
            Action::Button(GbaKey::Select),
            &[
                Char(' '),
                KeyCode::Backspace,
                KeyCode::Modifier(ModifierKeyCode::RightShift),
            ],
        );
        bind(&mut b, Action::Pause, &[Char('p')]);
        bind(&mut b, Action::Step, &[Char('.')]);
        bind(&mut b, Action::FastForward, &[KeyCode::Tab, Char('f')]);
        bind(&mut b, Action::Mute, &[Char('m')]);
        bind(&mut b, Action::SaveState, &[KeyCode::F(5)]);
        bind(&mut b, Action::LoadState, &[KeyCode::F(8)]);
        bind(&mut b, Action::States, &[KeyCode::F(2)]);
        for (action, button) in [
            (Action::Button(GbaKey::Up), PadButton::Up),
            (Action::Button(GbaKey::Down), PadButton::Down),
            (Action::Button(GbaKey::Left), PadButton::Left),
            (Action::Button(GbaKey::Right), PadButton::Right),
            (Action::Button(GbaKey::A), PadButton::East),
            (Action::Button(GbaKey::B), PadButton::South),
            (Action::Button(GbaKey::L), PadButton::L1),
            (Action::Button(GbaKey::R), PadButton::R1),
            (Action::Button(GbaKey::Start), PadButton::Start),
            (Action::Button(GbaKey::Select), PadButton::Select),
            (Action::FastForward, PadButton::R2),
            (Action::States, PadButton::L2),
            (Action::Pause, PadButton::North),
            (Action::Leave, PadButton::Mode),
            (Action::Help, PadButton::RightStick),
        ] {
            b.bind(Input::Pad(button), action);
        }
        b
    }

    /// Where the override file lives.
    #[must_use]
    pub fn file() -> Option<PathBuf> {
        config_dir().map(|dir| dir.join("keys"))
    }

    /// The defaults with the user's file applied. Problems in the file
    /// are returned as messages; the lines around them still count.
    #[must_use]
    pub fn load() -> (Self, Vec<String>) {
        match Self::file().and_then(|file| fs::read_to_string(file).ok()) {
            Some(text) => Self::parse(&text),
            None => (Self::defaults(), Vec::new()),
        }
    }

    /// Applies `text` (the file format above) on top of the defaults.
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut bindings = Self::defaults();
        let mut problems = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, keys)) = line.split_once('=') else {
                problems.push(format!("keys line {}: expected `button = key`", n + 1));
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let Some(action) = Action::from_name(&name) else {
                problems.push(format!("keys line {}: unknown button `{name}`", n + 1));
                continue;
            };
            let mut inputs = Vec::new();
            let mut words = 0;
            for word in keys.split(|c: char| c.is_whitespace() || c == ',') {
                if word.is_empty() {
                    continue;
                }
                words += 1;
                match parse_input(word) {
                    Some(input) => inputs.push(input),
                    None => problems.push(format!("keys line {}: unknown key `{word}`", n + 1)),
                }
            }
            if words == 0 {
                bindings.table.retain(|&(_, a)| a != action);
            }
            // Only the kinds the line names are replaced: a keyboard
            // layout written before pads existed keeps the pad defaults.
            bindings
                .table
                .retain(|&(i, a)| a != action || !inputs.iter().any(|&n| n.same_kind(i)));
            for input in inputs {
                bindings.bind(input, action);
            }
        }
        (bindings, problems)
    }

    /// The action for a key, if any. Letters match either case.
    #[must_use]
    pub fn action(&self, code: KeyCode) -> Option<Action> {
        self.lookup(Input::Key(code))
    }

    /// The first pad button bound to an action, for hints.
    #[must_use]
    pub fn pad_for(&self, action: Action) -> Option<PadButton> {
        self.inputs_for(action).find_map(|i| match i {
            Input::Pad(button) => Some(button),
            Input::Key(_) => None,
        })
    }

    /// The action for a pad button, if any.
    #[must_use]
    pub fn pad_action(&self, button: PadButton) -> Option<Action> {
        self.lookup(Input::Pad(button))
    }

    fn lookup(&self, input: Input) -> Option<Action> {
        let input = normalise(input);
        self.table
            .iter()
            .find_map(|&(i, a)| (i == input).then_some(a))
    }

    /// The inputs bound to an action: keys, then pad buttons, each in
    /// binding order.
    pub fn inputs_for(&self, action: Action) -> impl Iterator<Item = Input> + '_ {
        let of_kind = move |pad: bool| {
            self.table
                .iter()
                .filter(move |&&(i, a)| a == action && matches!(i, Input::Pad(_)) == pad)
                .map(|&(i, _)| i)
        };
        of_kind(false).chain(of_kind(true))
    }

    /// The inputs bound to an action, in binding order, as display names.
    #[must_use]
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        self.inputs_for(action).map(input_name).collect()
    }

    /// Binds an input, taking it away from whatever it did before.
    fn bind(&mut self, input: Input, action: Action) {
        let input = normalise(input);
        self.table.retain(|&(i, _)| i != input);
        self.table.push((input, action));
    }
}

/// Letters are compared lower-case so Shift does not change a binding.
const fn normalise(input: Input) -> Input {
    match input {
        Input::Key(KeyCode::Char(c)) => Input::Key(KeyCode::Char(c.to_ascii_lowercase())),
        other => other,
    }
}

/// Parses a name from the file: `pad:` and a [`PadButton`] name, or a
/// key as [`parse_key`] reads it.
fn parse_input(word: &str) -> Option<Input> {
    match word.split_once(':') {
        Some((prefix, button)) if prefix.eq_ignore_ascii_case("pad") => {
            PadButton::from_name(button).map(Input::Pad)
        }
        _ => parse_key(word).map(Input::Key),
    }
}

/// The file-format name of an input, also used in the help overlay.
pub fn input_name(input: Input) -> String {
    match input {
        Input::Key(code) => key_name(code),
        Input::Pad(button) => format!("pad:{}", button.name()),
    }
}

/// Parses a key name from the file: a single character, or one of the
/// names in [`key_name`] (case-insensitive).
fn parse_key(word: &str) -> Option<KeyCode> {
    let lower = word.to_ascii_lowercase();
    let mut chars = word.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c.to_ascii_lowercase()));
    }
    Some(match lower.as_str() {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "enter" | "return" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "insert" => KeyCode::Insert,
        "delete" => KeyCode::Delete,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "lshift" => KeyCode::Modifier(ModifierKeyCode::LeftShift),
        "rshift" => KeyCode::Modifier(ModifierKeyCode::RightShift),
        "lctrl" => KeyCode::Modifier(ModifierKeyCode::LeftControl),
        "rctrl" => KeyCode::Modifier(ModifierKeyCode::RightControl),
        "lalt" => KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        "ralt" => KeyCode::Modifier(ModifierKeyCode::RightAlt),
        f if f.starts_with('f') => {
            KeyCode::F(f[1..].parse().ok().filter(|n| (1..=12).contains(n))?)
        }
        _ => return None,
    })
}

/// The file-format name of a key, also used in the help overlay.
fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Modifier(ModifierKeyCode::LeftShift) => "lshift".into(),
        KeyCode::Modifier(ModifierKeyCode::RightShift) => "rshift".into(),
        KeyCode::Modifier(ModifierKeyCode::LeftControl) => "lctrl".into(),
        KeyCode::Modifier(ModifierKeyCode::RightControl) => "rctrl".into(),
        KeyCode::Modifier(ModifierKeyCode::LeftAlt) => "lalt".into(),
        KeyCode::Modifier(ModifierKeyCode::RightAlt) => "ralt".into(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

impl fmt::Display for Bindings {
    /// The table in file format, so the overlay can say what to edit.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for action in Action::ALL {
            writeln!(
                f,
                "{:<6} = {}",
                action.name(),
                self.keys_for(action).join(" ")
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_action() {
        let b = Bindings::defaults();
        for action in Action::ALL {
            assert!(!b.keys_for(action).is_empty(), "{action:?} has no key");
        }
        assert_eq!(
            b.action(KeyCode::Char('Z')),
            Some(Action::Button(GbaKey::A))
        );
        assert_eq!(
            b.action(KeyCode::Enter),
            Some(Action::Button(GbaKey::Start))
        );
        assert_eq!(b.action(KeyCode::Tab), Some(Action::FastForward));
        assert_eq!(b.action(KeyCode::Char('q')), None);
    }

    #[test]
    fn file_overrides_only_what_it_mentions() {
        let (b, problems) = Bindings::parse(
            "# my layout\n\
             a = j\n\
             b = k, u\n\
             select =\n\
             fast = F5   # comment\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            b.action(KeyCode::Char('j')),
            Some(Action::Button(GbaKey::A))
        );
        assert_eq!(b.action(KeyCode::Char('a')), None, "old key released");
        assert_eq!(
            b.keys_for(Action::Button(GbaKey::B)),
            ["k", "u", "pad:south"]
        );
        assert!(b.keys_for(Action::Button(GbaKey::Select)).is_empty());
        assert_eq!(b.action(KeyCode::F(5)), Some(Action::FastForward));
        assert_eq!(b.action(KeyCode::Tab), None);
        // Untouched actions keep their defaults.
        assert_eq!(
            b.action(KeyCode::Enter),
            Some(Action::Button(GbaKey::Start))
        );
    }

    #[test]
    fn a_key_belongs_to_one_action() {
        let (b, _) = Bindings::parse("l = a\n");
        assert_eq!(
            b.action(KeyCode::Char('a')),
            Some(Action::Button(GbaKey::L))
        );
        assert_eq!(b.keys_for(Action::Button(GbaKey::A)), ["z", "pad:east"]);
    }

    #[test]
    fn problems_are_reported_and_skipped() {
        let (b, problems) = Bindings::parse("nonsense\nfoo = a\na = bogus z\n");
        assert_eq!(problems.len(), 3);
        assert!(problems[0].contains("line 1"));
        assert!(problems[1].contains("unknown button `foo`"));
        assert!(problems[2].contains("unknown key `bogus`"));
        assert_eq!(
            b.keys_for(Action::Button(GbaKey::A)),
            ["z", "pad:east"],
            "valid key on the bad line kept"
        );
    }

    #[test]
    fn pad_buttons_bind_like_keys() {
        let b = Bindings::defaults();
        assert_eq!(
            b.pad_action(PadButton::East),
            Some(Action::Button(GbaKey::A))
        );
        assert_eq!(b.pad_action(PadButton::R2), Some(Action::FastForward));
        assert_eq!(b.pad_action(PadButton::Mode), Some(Action::Leave));
        assert_eq!(b.pad_action(PadButton::West), None, "free for the panel");

        let (b, problems) = Bindings::parse("a = PAD:West\nstart = pad:start enter\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            b.pad_action(PadButton::West),
            Some(Action::Button(GbaKey::A))
        );
        assert_eq!(b.pad_action(PadButton::East), None, "old button released");
        assert_eq!(
            b.keys_for(Action::Button(GbaKey::A)),
            ["a", "z", "pad:west"],
            "a pad-only line keeps the keys"
        );
        assert_eq!(
            b.keys_for(Action::Button(GbaKey::Start)),
            ["enter", "pad:start"]
        );
    }

    #[test]
    fn a_line_replaces_only_the_kinds_it_names() {
        let (b, _) = Bindings::parse("b = k\nl =\n");
        assert_eq!(b.keys_for(Action::Button(GbaKey::B)), ["k", "pad:south"]);
        assert!(b.keys_for(Action::Button(GbaKey::L)).is_empty());
        assert_eq!(b.pad_action(PadButton::L1), None);
    }

    #[test]
    fn unknown_pad_buttons_are_problems() {
        let (b, problems) = Bindings::parse("a = pad:a\n");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("`pad:a`"));
        assert_eq!(b, Bindings::defaults(), "nothing valid, nothing changed");
    }

    #[test]
    fn key_names_round_trip() {
        for name in [
            "a",
            ".",
            "up",
            "enter",
            "space",
            "backspace",
            "tab",
            "rshift",
            "f12",
            "pagedown",
        ] {
            let key = parse_key(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(key_name(key), name);
        }
        assert_eq!(parse_key("F0"), None);
        assert_eq!(parse_key("f13"), None);
        assert_eq!(parse_key("RETURN"), Some(KeyCode::Enter));
    }

    #[test]
    fn display_is_valid_input() {
        let text = Bindings::defaults().to_string();
        let (again, problems) = Bindings::parse(&text);
        assert!(problems.is_empty(), "{problems:?}");
        for action in Action::ALL {
            assert_eq!(
                again.keys_for(action),
                Bindings::defaults().keys_for(action)
            );
        }
    }
}
