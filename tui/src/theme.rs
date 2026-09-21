//! The frontend's palette: a dark indigo base with a lavender accent,
//! in the spirit of the original purple handheld.

use ratatui::style::{Color, Modifier, Style};

/// Page background.
pub const BG: Color = Color::Rgb(0x12, 0x10, 0x1C);
/// Slightly raised surfaces (selected rows, the footer).
pub const SURFACE: Color = Color::Rgb(0x1E, 0x1A, 0x2E);
/// Primary text.
pub const TEXT: Color = Color::Rgb(0xE8, 0xE4, 0xF4);
/// Secondary text and borders.
pub const DIM: Color = Color::Rgb(0x6F, 0x68, 0x88);
/// Accent: highlights, the logo, key hints.
pub const ACCENT: Color = Color::Rgb(0xA8, 0x96, 0xFF);
/// A warmer accent for "good" states (save present, valid header).
pub const OK: Color = Color::Rgb(0x8C, 0xE0, 0xA0);
/// Warnings (missing header, unreadable folder).
pub const WARN: Color = Color::Rgb(0xF0, 0xB8, 0x6C);

/// Plain body text on the page background.
#[must_use]
pub fn text() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

/// Secondary text.
#[must_use]
pub fn dim() -> Style {
    Style::default().fg(DIM).bg(BG)
}

/// Accent text.
#[must_use]
pub fn accent() -> Style {
    Style::default().fg(ACCENT).bg(BG)
}

/// Borders and pane titles.
#[must_use]
pub fn border() -> Style {
    Style::default().fg(DIM).bg(BG)
}

/// The selected row of a focused list.
#[must_use]
pub fn selected() -> Style {
    Style::default()
        .fg(BG)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

/// The selected row of an unfocused list.
#[must_use]
pub fn selected_unfocused() -> Style {
    Style::default().fg(TEXT).bg(SURFACE)
}

/// A key name inside a hint such as `⏎ play`.
#[must_use]
pub fn key() -> Style {
    Style::default()
        .fg(ACCENT)
        .bg(SURFACE)
        .add_modifier(Modifier::BOLD)
}

/// The label part of a hint.
#[must_use]
pub fn hint() -> Style {
    Style::default().fg(DIM).bg(SURFACE)
}
