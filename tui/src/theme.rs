//! The frontend palette and user-configurable colour themes.
use std::fs;
use std::sync::OnceLock;

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
            ok: Color::Green,
            warn: Color::Yellow,
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
            "OK" => self.ok = value,
            "WARN" => self.warn = value,
            _ => return false,
        }
        true
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// The palette loaded once at application startup.
#[must_use]
pub fn palette() -> &'static Palette {
    PALETTE.get_or_init(Palette::dark)
}

/// Loads the optional `theme` file. Invalid lines are reported but do not stop startup.
pub fn load() -> Vec<String> {
    let Some(path) = config_dir().map(|dir| dir.join("theme")) else {
        let _ = PALETTE.set(Palette::dark());
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(path) else {
        let _ = PALETTE.set(Palette::dark());
        return Vec::new();
    };

    let mut palette = Palette::dark();
    let mut problems = Vec::new();
    let mut saw_colour_override = false;
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
            if saw_colour_override {
                problems.push(format!(
                    "theme line {}: preset resets colour overrides; put preset before colours",
                    index + 1
                ));
            }
            palette = match value.to_ascii_lowercase().as_str() {
                "dark" => Palette::dark(),
                "light" => Palette::light(),
                "terminal" => Palette::terminal(),
                _ => {
                    problems.push(format!(
                        "theme line {}: unknown preset `{value}`",
                        index + 1
                    ));
                    continue;
                }
            };
            continue;
        }
        let Some(color) = parse_color(value) else {
            problems.push(format!(
                "theme line {}: expected a colour like `#rrggbb`",
                index + 1
            ));
            continue;
        };
        saw_colour_override = true;
        if !palette.set(&name, color) {
            problems.push(format!("theme line {}: unknown colour `{name}`", index + 1));
        }
    }
    let _ = PALETTE.set(palette);
    problems
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

/// Plain body text on the page background.
#[must_use]
pub fn text() -> Style {
    let p = palette();
    Style::default().fg(p.text).bg(p.bg)
}

/// Secondary text.
#[must_use]
pub fn dim() -> Style {
    let p = palette();
    Style::default().fg(p.dim).bg(p.bg)
}

/// Accent text.
#[must_use]
pub fn accent() -> Style {
    let p = palette();
    Style::default().fg(p.accent).bg(p.bg)
}

/// Borders and pane titles.
#[must_use]
pub fn border() -> Style {
    let p = palette();
    Style::default().fg(p.dim).bg(p.bg)
}

/// The selected row of a focused list.
#[must_use]
pub fn selected() -> Style {
    let p = palette();
    Style::default()
        .fg(p.bg)
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
    fn presets_keep_hints_and_selection_readable() {
        for p in [Palette::dark(), Palette::light(), Palette::terminal()] {
            assert_ne!(p.dim, p.surface, "{p:?}");
            assert_ne!(p.text, p.surface, "{p:?}");
            assert_ne!(p.bg, p.accent, "{p:?}");
        }
    }
}
