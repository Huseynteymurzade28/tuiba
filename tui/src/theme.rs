//! The frontend palette and user-configurable colour themes.
use std::fs;
use std::io;
use std::sync::{PoisonError, RwLock};

use ratatui::style::{Color, Modifier, Style};

use crate::library::config_dir;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent_light: Color,
    /// Text drawn on the accent, such as the selected row.
    pub on_accent: Color,
    pub ok: Color,
    pub warn: Color,
}

impl Palette {
    const fn dark() -> Self {
        Self {
            bg: Color::Rgb(0x12, 0x10, 0x1C),
            surface: Color::Rgb(0x1E, 0x1A, 0x2E),
            text: Color::Rgb(0xE8, 0xE4, 0xF4),
            dim: Color::Rgb(0x6F, 0x68, 0x88),
            accent: Color::Rgb(0xA8, 0x96, 0xFF),
            accent_light: Color::Rgb(0xC4, 0xB6, 0xFF),
            on_accent: Color::Rgb(0x12, 0x10, 0x1C),
            ok: Color::Rgb(0x8C, 0xE0, 0xA0),
            warn: Color::Rgb(0xF0, 0xB8, 0x6C),
        }
    }

    const fn light() -> Self {
        Self {
            bg: Color::Rgb(0xF7, 0xF5, 0xFC),
            surface: Color::Rgb(0xE9, 0xE4, 0xF2),
            text: Color::Rgb(0x25, 0x20, 0x32),
            dim: Color::Rgb(0x68, 0x60, 0x78),
            accent: Color::Rgb(0x63, 0x45, 0xC7),
            accent_light: Color::Rgb(0x82, 0x66, 0xDE),
            on_accent: Color::Rgb(0xF7, 0xF5, 0xFC),
            ok: Color::Rgb(0x1B, 0x78, 0x3A),
            warn: Color::Rgb(0x9A, 0x54, 0x00),
        }
    }

    const fn terminal() -> Self {
        Self {
            bg: Color::Reset,
            surface: Color::DarkGray,
            text: Color::Reset,
            // Hints are dim text on the surface, so dim must differ from it.
            dim: Color::Gray,
            accent: Color::Cyan,
            accent_light: Color::LightCyan,
            // The terminal's own foreground may be light or dark; on cyan
            // only black is sure to read.
            on_accent: Color::Black,
            ok: Color::Green,
            warn: Color::Yellow,
        }
    }

    /// The four greens of the original Game Boy screen.
    const fn dmg() -> Self {
        Self {
            bg: Color::Rgb(0x0F, 0x1B, 0x0F),
            surface: Color::Rgb(0x1B, 0x2D, 0x19),
            text: Color::Rgb(0xD0, 0xE4, 0xA4),
            dim: Color::Rgb(0x62, 0x80, 0x4A),
            accent: Color::Rgb(0x9B, 0xBC, 0x0F),
            accent_light: Color::Rgb(0xC6, 0xE0, 0x4E),
            on_accent: Color::Rgb(0x0F, 0x1B, 0x0F),
            ok: Color::Rgb(0x7E, 0xD9, 0x8C),
            warn: Color::Rgb(0xE8, 0xB8, 0x5A),
        }
    }

    /// Icy blue.
    const fn glacier() -> Self {
        Self {
            bg: Color::Rgb(0x0D, 0x15, 0x1F),
            surface: Color::Rgb(0x17, 0x23, 0x31),
            text: Color::Rgb(0xE2, 0xEE, 0xF8),
            dim: Color::Rgb(0x5E, 0x78, 0x91),
            accent: Color::Rgb(0x7C, 0xC4, 0xF0),
            accent_light: Color::Rgb(0xAE, 0xE0, 0xFF),
            on_accent: Color::Rgb(0x0D, 0x15, 0x1F),
            ok: Color::Rgb(0x8C, 0xE0, 0xB4),
            warn: Color::Rgb(0xF0, 0xC0, 0x70),
        }
    }

    /// Warm orange.
    const fn spice() -> Self {
        Self {
            bg: Color::Rgb(0x19, 0x11, 0x0C),
            surface: Color::Rgb(0x29, 0x1C, 0x14),
            text: Color::Rgb(0xF6, 0xE7, 0xDA),
            dim: Color::Rgb(0x8A, 0x6D, 0x5A),
            accent: Color::Rgb(0xFF, 0x9A, 0x4D),
            accent_light: Color::Rgb(0xFF, 0xBE, 0x85),
            on_accent: Color::Rgb(0x19, 0x11, 0x0C),
            ok: Color::Rgb(0xA8, 0xD8, 0x78),
            warn: Color::Rgb(0xFF, 0x6B, 0x6B),
        }
    }

    /// Hot pink.
    const fn fuchsia() -> Self {
        Self {
            bg: Color::Rgb(0x19, 0x0E, 0x18),
            surface: Color::Rgb(0x29, 0x17, 0x27),
            text: Color::Rgb(0xF7, 0xE4, 0xF3),
            dim: Color::Rgb(0x86, 0x6A, 0x82),
            accent: Color::Rgb(0xFF, 0x6A, 0xC1),
            accent_light: Color::Rgb(0xFF, 0x9F, 0xD8),
            on_accent: Color::Rgb(0x19, 0x0E, 0x18),
            ok: Color::Rgb(0x8C, 0xE0, 0xA0),
            warn: Color::Rgb(0xF0, 0xB8, 0x6C),
        }
    }

    fn set(&mut self, key: &str, value: Color) -> bool {
        match key {
            "BG" => self.bg = value,
            "SURFACE" => self.surface = value,
            "TEXT" => self.text = value,
            "DIM" => self.dim = value,
            "ACCENT" => self.accent = value,
            "ACCENT_LIGHT" => self.accent_light = value,
            "ON_ACCENT" => self.on_accent = value,
            "OK" => self.ok = value,
            "WARN" => self.warn = value,
            _ => return false,
        }
        true
    }
}

/// The built-in palettes a `theme` file (or the picker behind `t` in the
/// library) chooses from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Dark,
    Light,
    Terminal,
    Dmg,
    Glacier,
    Spice,
    Fuchsia,
}

impl Preset {
    /// Every preset, in the order the picker lists them.
    pub const ALL: [Self; 7] = [
        Self::Dark,
        Self::Light,
        Self::Terminal,
        Self::Dmg,
        Self::Glacier,
        Self::Spice,
        Self::Fuchsia,
    ];

    fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }

    /// The name the `theme` file uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Terminal => "terminal",
            Self::Dmg => "dmg",
            Self::Glacier => "glacier",
            Self::Spice => "spice",
            Self::Fuchsia => "fuchsia",
        }
    }

    /// A few words on what it looks like.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Dark => "indigo and lavender",
            Self::Light => "lavender on paper",
            Self::Terminal => "your terminal's own colours",
            Self::Dmg => "the first Game Boy's greens",
            Self::Glacier => "icy blue",
            Self::Spice => "warm orange",
            Self::Fuchsia => "hot pink",
        }
    }

    /// Its colours, before any of the `theme` file's own.
    #[must_use]
    pub const fn palette(self) -> Palette {
        match self {
            Self::Dark => Palette::dark(),
            Self::Light => Palette::light(),
            Self::Terminal => Palette::terminal(),
            Self::Dmg => Palette::dmg(),
            Self::Glacier => Palette::glacier(),
            Self::Spice => Palette::spice(),
            Self::Fuchsia => Palette::fuchsia(),
        }
    }
}

/// The preset in use and the file's own colours laid over it, kept so
/// switching presets keeps those colours just as reloading the file would.
struct Theme {
    preset: Preset,
    overrides: Vec<(String, Color)>,
    palette: Palette,
}

static THEME: RwLock<Theme> = RwLock::new(Theme {
    preset: Preset::Dark,
    overrides: Vec::new(),
    palette: Palette::dark(),
});

/// The palette in use.
#[must_use]
pub fn palette() -> Palette {
    THEME.read().unwrap_or_else(PoisonError::into_inner).palette
}

/// Loads the optional `theme` file. Invalid lines are reported but do not stop startup.
pub fn load() -> Vec<String> {
    let Some(contents) = config_dir().and_then(|dir| fs::read_to_string(dir.join("theme")).ok())
    else {
        return Vec::new();
    };
    let (theme, problems) = parse(&contents);
    *THEME.write().unwrap_or_else(PoisonError::into_inner) = theme;
    problems
}

fn parse(contents: &str) -> (Theme, Vec<String>) {
    let mut preset = Preset::Dark;
    let mut overrides = Vec::new();
    let mut problems = Vec::new();
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            problems.push(format!("theme line {}: expected `name = value`", index + 1));
            continue;
        };
        let name = name.trim().to_ascii_uppercase();
        let value = value.trim();
        if name == "PRESET" {
            let Some(chosen) = Preset::parse(value) else {
                problems.push(format!(
                    "theme line {}: unknown preset `{value}`",
                    index + 1
                ));
                continue;
            };
            if !overrides.is_empty() {
                problems.push(format!(
                    "theme line {}: preset resets colour overrides; put preset before colours",
                    index + 1
                ));
                overrides.clear();
            }
            preset = chosen;
            continue;
        }
        let Some(color) = parse_color(value) else {
            problems.push(format!(
                "theme line {}: expected a colour like `#rrggbb`",
                index + 1
            ));
            continue;
        };
        if Palette::dark().set(&name, color) {
            overrides.push((name, color));
        } else {
            problems.push(format!("theme line {}: unknown colour `{name}`", index + 1));
        }
    }
    (Theme::new(preset, overrides), problems)
}

impl Theme {
    fn new(preset: Preset, overrides: Vec<(String, Color)>) -> Self {
        let mut palette = preset.palette();
        for (name, color) in &overrides {
            palette.set(name, *color);
        }
        Self {
            preset,
            overrides,
            palette,
        }
    }
}

/// The preset in use.
#[must_use]
pub fn current() -> Preset {
    THEME.read().unwrap_or_else(PoisonError::into_inner).preset
}

/// Switches to `preset`, keeping the file's own colours, without saving:
/// the picker shows each preset this way as the cursor passes it.
pub fn preview(preset: Preset) {
    let mut theme = THEME.write().unwrap_or_else(PoisonError::into_inner);
    let overrides = std::mem::take(&mut theme.overrides);
    *theme = Theme::new(preset, overrides);
}

/// Switches to `preset` and writes it to the `theme` file so it sticks.
/// The preset is in use even when saving fails.
///
/// # Errors
///
/// The `theme` file could not be read or written.
pub fn choose(preset: Preset) -> io::Result<()> {
    preview(preset);
    save_preset(preset)
}

fn save_preset(preset: Preset) -> io::Result<()> {
    let dir = config_dir().ok_or_else(|| io::Error::other("no config directory"))?;
    let path = dir.join("theme");
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    fs::create_dir_all(&dir)?;
    fs::write(path, with_preset(&contents, preset))
}

/// `contents` with every `preset` line naming `preset`, or one added at
/// the top: the file then loads exactly as the palette now looks.
fn with_preset(contents: &str, preset: Preset) -> String {
    let line = format!("preset = {}", preset.name());
    let mut found = false;
    let mut out = String::new();
    for raw in contents.lines() {
        let is_preset = raw
            .split_once('=')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("preset"))
            && !raw.trim_start().starts_with('#');
        if is_preset {
            found = true;
            out.push_str(&line);
        } else {
            out.push_str(raw);
        }
        out.push('\n');
    }
    if found { out } else { format!("{line}\n{out}") }
}

fn parse_color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if !hex.is_ascii() || hex.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

/// `from` moved `t` (0–1) of the way to `to`. Only RGB colours blend;
/// the named colours of the terminal preset switch over at the halfway
/// point, so gradients there fall back to two flat halves.
#[must_use]
pub fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let lerp =
                |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
            Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
        }
        _ if t < 0.5 => from,
        _ => to,
    }
}

/// The page itself: what a screen is cleared to before anything is
/// drawn on it.
#[must_use]
pub fn page() -> Style {
    let p = palette();
    Style::default().fg(p.text).bg(p.bg)
}

// The text styles below set only a colour, so they read the same on the
// page and inside a panel's raised surface.

/// Plain body text.
#[must_use]
pub fn text() -> Style {
    Style::default().fg(palette().text)
}

/// Secondary text.
#[must_use]
pub fn dim() -> Style {
    Style::default().fg(palette().dim)
}

/// Accent text.
#[must_use]
pub fn accent() -> Style {
    Style::default().fg(palette().accent)
}

/// Borders and pane titles.
#[must_use]
pub fn border() -> Style {
    Style::default().fg(palette().dim)
}

/// The selected row of a focused list.
#[must_use]
pub fn selected() -> Style {
    let p = palette();
    Style::default()
        .fg(p.on_accent)
        .bg(p.accent)
        .add_modifier(Modifier::BOLD)
}

/// The selected row of an unfocused list.
#[must_use]
pub fn selected_unfocused() -> Style {
    let p = palette();
    Style::default().fg(p.text).bg(p.surface)
}

/// A key name inside a hint such as `⏎ play`.
#[must_use]
pub fn key() -> Style {
    let p = palette();
    Style::default()
        .fg(p.accent)
        .bg(p.surface)
        .add_modifier(Modifier::BOLD)
}

/// The label part of a hint.
#[must_use]
pub fn hint() -> Style {
    let p = palette();
    Style::default().fg(p.dim).bg(p.surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_hex_colours() {
        assert_eq!(parse_color("#a896ff"), Some(Color::Rgb(0xA8, 0x96, 0xFF)));
        assert_eq!(parse_color("#xyzxyz"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#€abc"), None);
    }

    #[test]
    fn presets_have_expected_backgrounds() {
        assert_eq!(Palette::dark().bg, Color::Rgb(0x12, 0x10, 0x1C));
        assert_eq!(Palette::terminal().bg, Color::Reset);
    }

    #[test]
    fn colours_after_the_preset_survive_switching() {
        let (theme, problems) = parse("preset = light\nACCENT = #ff0000\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(theme.preset, Preset::Light);
        assert_eq!(theme.palette.accent, Color::Rgb(0xFF, 0, 0));
        let next = Theme::new(Preset::Terminal, theme.overrides);
        assert_eq!(next.preset, Preset::Terminal);
        assert_eq!(next.palette.accent, Color::Rgb(0xFF, 0, 0));
        assert_eq!(next.palette.bg, Color::Reset);
    }

    #[test]
    fn a_later_preset_drops_earlier_colours() {
        let (theme, problems) = parse("ACCENT = #ff0000\npreset = dark\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(theme.palette.accent, Palette::dark().accent);
    }

    #[test]
    fn saving_a_preset_rewrites_or_adds_its_line() {
        assert_eq!(with_preset("", Preset::Light), "preset = light\n");
        assert_eq!(
            with_preset("# mine\nPreset = dark\nOK = #00ff00", Preset::Terminal),
            "# mine\npreset = terminal\nOK = #00ff00\n"
        );
        assert_eq!(
            with_preset("# preset = dark\nOK = #00ff00\n", Preset::Light),
            "preset = light\n# preset = dark\nOK = #00ff00\n"
        );
        let (theme, _) = parse(&with_preset("OK = #00ff00\n", Preset::Light));
        assert_eq!(theme.preset, Preset::Light);
        assert_eq!(theme.palette.ok, Color::Rgb(0, 0xFF, 0));
    }

    #[test]
    fn mixing_blends_rgb_and_switches_named_colours() {
        let (a, b) = (Color::Rgb(0, 100, 200), Color::Rgb(100, 200, 0));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 0.5), Color::Rgb(50, 150, 100));
        assert_eq!(mix(a, b, 7.0), b);
        assert_eq!(mix(Color::Cyan, Color::Gray, 0.4), Color::Cyan);
        assert_eq!(mix(Color::Cyan, Color::Gray, 0.6), Color::Gray);
    }

    #[test]
    fn presets_keep_hints_and_selection_readable() {
        for p in Preset::ALL.map(Preset::palette) {
            assert_ne!(p.dim, p.surface, "{p:?}");
            assert_ne!(p.text, p.surface, "{p:?}");
            assert_ne!(p.bg, p.accent, "{p:?}");
            assert_ne!(p.on_accent, p.accent, "{p:?}");
            // Text on the accent must not be the terminal's own colour,
            // which may be as light as the accent.
            assert_ne!(p.on_accent, Color::Reset, "{p:?}");
        }
    }

    #[test]
    fn every_preset_parses_by_name() {
        for preset in Preset::ALL {
            assert_eq!(Preset::parse(preset.name()), Some(preset));
            assert_eq!(Preset::parse(&preset.name().to_uppercase()), Some(preset));
        }
        assert_eq!(Preset::parse("nope"), None);
    }
}
