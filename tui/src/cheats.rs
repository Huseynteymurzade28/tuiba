//! Cheats: the `.cht` file next to a ROM, and the panel that turns its
//! codes on and off.
//!
//! The file uses libretro's format, so the cheat files `RetroArch` users
//! share (and libretro-database collects) work after a rename to
//! `<rom>.cht`:
//!
//! ```text
//! cheats = 2
//!
//! cheat0_desc = "Infinite money"
//! cheat0_code = "82025BC4+270F"
//! cheat0_enable = true
//!
//! cheat1_desc = "Master code"
//! cheat1_code = "D8BAE4D9+4864DCE5+A86CDBA5+19BA49B3"
//! cheat1_enable = false
//! ```
//!
//! A code is its lines joined by `+`; the line breaks are recovered from
//! the shape of the parts, and the format is detected per cheat (see
//! [`tuiba_core::cheats`]). Turning a cheat on or off rewrites only its
//! `enable` line, so anything else in the file — keys tuiba does not use
//! included — stays as it was.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use tuiba_core::Gba;
use tuiba_core::cheats::{Cheat, Format};

use crate::gamepad::{PadButton, PadStyle};
use crate::theme;

/// One cheat of the file.
pub struct Entry {
    /// What the file calls it.
    pub name: String,
    pub enabled: bool,
    /// The decoded code, or why it could not be decoded.
    pub cheat: Result<Cheat, String>,
    /// Index of its `enable` line in [`CheatFile::lines`], if it has one.
    enable_line: Option<usize>,
    /// Index of the line after which a missing `enable` line goes.
    last_line: usize,
    /// The number in its keys (`cheat<n>_…`).
    number: usize,
}

/// The cheats of one game.
pub struct CheatFile {
    path: PathBuf,
    /// The file as read, line by line, so a toggle can rewrite one line.
    lines: Vec<String>,
    entries: Vec<Entry>,
}

impl CheatFile {
    /// Where the cheats for `rom` live.
    #[must_use]
    pub fn path_for(rom: &Path) -> PathBuf {
        rom.with_extension("cht")
    }

    /// No cheats, to be kept at the usual place for `rom`.
    #[must_use]
    pub fn empty(rom: &Path) -> Self {
        Self::parse(Self::path_for(rom), "")
    }

    /// Reads the cheats for `rom`. A missing file is an empty list.
    ///
    /// # Errors
    ///
    /// When the file exists but cannot be read.
    pub fn load(rom: &Path) -> io::Result<Self> {
        let path = Self::path_for(rom);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        Ok(Self::parse(path, &text))
    }

    fn parse(path: PathBuf, text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        // Keyed by number, so the file's order of keys does not matter.
        let mut found: BTreeMap<usize, Raw> = BTreeMap::new();
        for (index, line) in lines.iter().enumerate() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some((number, field)) = key
                .trim()
                .strip_prefix("cheat")
                .and_then(|rest| rest.split_once('_'))
            else {
                continue;
            };
            let Ok(number) = number.parse::<usize>() else {
                continue;
            };
            let raw = found.entry(number).or_default();
            raw.last_line = raw.last_line.max(index);
            let value = unquote(value);
            match field {
                "desc" => raw.name = Some(value.to_owned()),
                "code" => raw.code = Some(value.to_owned()),
                "enable" => {
                    raw.enabled = value.eq_ignore_ascii_case("true");
                    raw.enable_line = Some(index);
                }
                _ => {}
            }
        }
        let entries = found
            .into_iter()
            .filter_map(|(number, raw)| {
                let code = raw.code?;
                Some(Entry {
                    name: raw.name.unwrap_or_else(|| format!("cheat {number}")),
                    enabled: raw.enabled,
                    cheat: Cheat::parse(&code_lines(&code), None).map_err(|e| e.to_string()),
                    enable_line: raw.enable_line,
                    last_line: raw.last_line,
                    number,
                })
            })
            .collect();
        Self {
            path,
            lines,
            entries,
        }
    }

    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cheats, in the file's order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many cheats are on and working.
    #[must_use]
    pub fn active(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.enabled && e.cheat.is_ok())
            .count()
    }

    /// Turns cheat `index` on or off and writes the file. The change
    /// stays in effect for this session even if the write fails.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn toggle(&mut self, index: usize) -> io::Result<()> {
        let Some(entry) = self.entries.get_mut(index) else {
            return Ok(());
        };
        entry.enabled = !entry.enabled;
        let line = format!("cheat{}_enable = {}", entry.number, entry.enabled);
        if let Some(at) = entry.enable_line {
            self.lines[at] = line;
        } else {
            let at = entry.last_line + 1;
            self.lines.insert(at, line);
            // Every line index from here on moved down by one.
            for other in &mut self.entries {
                for index in other.enable_line.iter_mut().chain([&mut other.last_line]) {
                    if *index >= at {
                        *index += 1;
                    }
                }
            }
            let entry = &mut self.entries[index];
            entry.enable_line = Some(at);
            entry.last_line = entry.last_line.max(at);
        }
        self.write()
    }

    /// Writes the lines out, through a temporary file so a crash
    /// mid-write cannot cost the user their list.
    fn write(&self) -> io::Result<()> {
        let mut text = self.lines.join("\n");
        text.push('\n');
        let tmp = self.path.with_extension("cht.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.path)
    }

    /// Runs every enabled cheat once.
    pub fn apply(&self, gba: &mut Gba) {
        for entry in &self.entries {
            if entry.enabled
                && let Ok(cheat) = &entry.cheat
            {
                cheat.apply(gba);
            }
        }
    }
}

/// One cheat's keys, as collected from the file.
#[derive(Default)]
struct Raw {
    name: Option<String>,
    code: Option<String>,
    enabled: bool,
    enable_line: Option<usize>,
    last_line: usize,
}

/// A value with its quotes, if any, taken off.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// Turns a `+`-joined code back into lines: an eight-digit part pairs up
/// with the part after it when that one is eight (GameShark, Action
/// Replay) or four (CodeBreaker) digits long; anything else — a raw
/// `AAAAAAAA:VV`, a line already whole — stands alone.
fn code_lines(code: &str) -> String {
    let parts: Vec<&str> = code
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        let pairs_with = |next: &str| matches!(next.len(), 4 | 8) && !next.contains(':');
        if part.len() == 8
            && !part.contains(':')
            && let Some(next) = parts.get(i + 1).filter(|n| pairs_with(n))
        {
            lines.push(format!("{part} {next}"));
            i += 2;
        } else {
            lines.push(part.to_owned());
            i += 1;
        }
    }
    lines.join("\n")
}

/// What the panel wants done after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Turn the cheat at this index on or off.
    Toggle(usize),
    /// Close the panel.
    Close,
}

/// The cheat list over a paused game. It only keeps the cursor; the
/// list itself stays with the caller, who applies the cheats.
pub struct CheatsPanel {
    selected: usize,
    /// First row shown, for lists longer than the panel.
    scroll: usize,
}

/// Rows of cheats the panel shows at most.
const MAX_ROWS: usize = 16;

/// Panel width, in cells, when the terminal has room.
const WIDTH: u16 = 64;

impl CheatsPanel {
    #[must_use]
    pub const fn open() -> Self {
        Self {
            selected: 0,
            scroll: 0,
        }
    }

    /// Moves the cursor and turns the action keys into an [`Action`].
    /// Everything else is swallowed: the game must not see the keyboard
    /// while this is up.
    pub fn handle(&mut self, key: KeyEvent, count: usize) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(MAX_ROWS),
            KeyCode::PageDown => self.selected = (self.selected + MAX_ROWS).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Enter | KeyCode::Char(' ') if count > 0 => {
                return Some(Action::Toggle(self.selected));
            }
            KeyCode::Esc => return Some(Action::Close),
            _ => {}
        }
        None
    }

    /// The key a pad button stands for in the panel: the D-pad moves,
    /// the shoulders page, the pad's confirm button toggles and its back
    /// button closes.
    #[must_use]
    pub fn pad_key(button: PadButton, style: PadStyle) -> Option<KeyCode> {
        Some(match button {
            PadButton::Up => KeyCode::Up,
            PadButton::Down => KeyCode::Down,
            PadButton::L1 => KeyCode::PageUp,
            PadButton::R1 => KeyCode::PageDown,
            b if b == style.confirm() => KeyCode::Enter,
            b if b == style.back() => KeyCode::Esc,
            _ => return None,
        })
    }

    /// Draws the panel centred in `area`. With `pad`, the footer names
    /// that pad's buttons instead of keys.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, file: &CheatFile, pad: Option<PadStyle>) {
        let entries = file.entries();
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let width = WIDTH.min(area.width);
        // Borders, a gap, the two detail lines and the footer.
        let chrome = 2 + 3 + 1;
        let rows = if entries.is_empty() {
            4
        } else {
            entries
                .len()
                .min(MAX_ROWS)
                .min(usize::from(area.height.saturating_sub(chrome)))
                .max(1)
        };
        let height = (rows as u16 + chrome).min(area.height);
        let [popup] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(popup);

        frame.render_widget(Clear, popup);
        let on = file.active();
        let title = if on == 0 {
            " CHEATS ".to_owned()
        } else {
            format!(" CHEATS · {on} on ")
        };
        let block = Block::default()
            .title(Line::styled(title, theme::accent().bold()))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border())
            .padding(Padding::horizontal(1))
            .style(theme::text().bg(theme::SURFACE));
        let inside = block.inner(popup);
        frame.render_widget(block, popup);
        let [list, _, detail, footer] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inside);

        if entries.is_empty() {
            let file_name = file
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let lines = vec![
                Line::styled(
                    "No cheats for this game yet.",
                    theme::text().bg(theme::SURFACE),
                ),
                Line::default(),
                Line::from(vec![
                    Span::styled("Put them in ", theme::hint()),
                    Span::styled(file_name, theme::key()),
                    Span::styled(" next to the ROM.", theme::hint()),
                ]),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), list);
            let help = "libretro's .cht format; the codes can be raw, GameShark, \
                        Action Replay or CodeBreaker.";
            frame.render_widget(
                Paragraph::new(Line::styled(help, theme::hint())).wrap(Wrap { trim: true }),
                detail,
            );
            frame.render_widget(footer_hints(&[("esc", Hint::Back, "close")], pad), footer);
            return;
        }

        let rows = usize::from(list.height).max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let lines: Vec<Line> = entries
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(rows)
            .map(|(index, entry)| row(entry, index == self.selected, usize::from(list.width)))
            .collect();
        frame.render_widget(Paragraph::new(lines), list);

        let entry = &entries[self.selected];
        frame.render_widget(
            Paragraph::new(details(entry)).wrap(Wrap { trim: true }),
            detail,
        );
        let toggle = if entry.enabled { "turn off" } else { "turn on" };
        frame.render_widget(
            footer_hints(
                &[("⏎", Hint::Confirm, toggle), ("esc", Hint::Back, "close")],
                pad,
            ),
            footer,
        );
    }
}

/// One cheat as a list row: cursor, box, name, and its format on the
/// right.
fn row(entry: &Entry, focused: bool, width: usize) -> Line<'static> {
    let mark = match (&entry.cheat, entry.enabled) {
        (Err(_), _) => "[!]",
        (Ok(_), true) => "[✓]",
        (Ok(_), false) => "[ ]",
    };
    let tag = match &entry.cheat {
        Ok(cheat) => short_format(cheat),
        Err(_) => "?",
    };
    let cursor = if focused { "▸" } else { " " };
    let head = format!("{cursor} {mark} ");
    let room = width.saturating_sub(head.chars().count() + tag.len() + 1);
    let name: String = if entry.name.chars().count() > room {
        let mut cut: String = entry.name.chars().take(room.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        entry.name.clone()
    };
    let gap = room.saturating_sub(name.chars().count()) + 1;
    let base = Style::default().bg(theme::SURFACE);
    let style = match (&entry.cheat, entry.enabled, focused) {
        (_, _, true) => theme::selected(),
        (Err(_), _, _) => base.fg(theme::WARN),
        (Ok(_), true, _) => base.fg(theme::OK),
        (Ok(_), false, _) => base.fg(theme::DIM),
    };
    Line::from(vec![
        Span::styled(format!("{head}{name}{}", " ".repeat(gap)), style),
        Span::styled(
            tag.to_owned(),
            if focused { style } else { base.fg(theme::DIM) },
        ),
    ])
}

/// The format's tag in the list.
fn short_format(cheat: &Cheat) -> &'static str {
    match cheat.format() {
        Format::Raw => "raw",
        Format::GameShark => "GS",
        Format::ActionReplay => "AR",
        Format::CodeBreaker => "CB",
    }
}

/// What the selected cheat is, or why it does not work.
fn details(entry: &Entry) -> Vec<Line<'static>> {
    let base = Style::default().bg(theme::SURFACE);
    match &entry.cheat {
        Err(err) => vec![Line::styled(
            format!("Not usable: {err}"),
            base.fg(theme::WARN),
        )],
        Ok(cheat) => {
            let plain = match cheat.format() {
                Format::GameShark | Format::ActionReplay if !cheat.encrypted() => " (unencrypted)",
                _ => "",
            };
            let mut lines = vec![Line::styled(
                capitalised(&format!("{} code{plain}", cheat.format())),
                theme::hint(),
            )];
            if !cheat.skipped().is_empty() {
                lines.push(Line::styled(
                    format!("Skips {}: not possible here.", cheat.skipped().join(", ")),
                    base.fg(theme::WARN),
                ));
            } else if cheat.is_empty() {
                lines.push(Line::styled(
                    "Does nothing here — a master code is not needed in tuiba.",
                    theme::hint(),
                ));
            }
            lines
        }
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Which pad button a hint stands for.
#[derive(Clone, Copy)]
enum Hint {
    Confirm,
    Back,
}

fn footer_hints(hints: &[(&str, Hint, &str)], pad: Option<PadStyle>) -> Paragraph<'static> {
    let style = pad.unwrap_or(PadStyle::Generic);
    let mut spans = Vec::new();
    for &(key, hint, label) in hints {
        let key = if pad.is_some() {
            style.label(match hint {
                Hint::Confirm => style.confirm(),
                Hint::Back => style.back(),
            })
        } else {
            key
        };
        spans.push(Span::styled(format!(" {key} "), theme::key()));
        spans.push(Span::styled(format!("{label}  "), theme::hint()));
    }
    Paragraph::new(Line::from(spans)).alignment(Alignment::Center)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "cheats = 3

cheat0_desc = \"Master Code\"
cheat0_code = \"D8BAE4D9+4864DCE5+A86CDBA5+19BA49B3\"
cheat0_enable = false

cheat1_desc = \"No Random Battle\"
cheat1_code = \"320375D4+0000\"
cheat1_enable = true
cheat1_handler = 0

cheat2_desc = \"Raw\"
cheat2_code = \"02000000:12+02000002:3456\"
";

    #[test]
    fn joined_codes_split_back_into_lines() {
        assert_eq!(
            code_lines("D8BAE4D9+4864DCE5+A86CDBA5+19BA49B3"),
            "D8BAE4D9 4864DCE5\nA86CDBA5 19BA49B3"
        );
        assert_eq!(
            code_lines("320375D4+0000+82000002+BEEF"),
            "320375D4 0000\n82000002 BEEF"
        );
        assert_eq!(
            code_lines("A745569AA9B6+FD228345FE5F"),
            "A745569AA9B6\nFD228345FE5F"
        );
        assert_eq!(
            code_lines("02000000:12+02000002:3456"),
            "02000000:12\n02000002:3456"
        );
    }

    #[test]
    fn libretro_files_are_read() {
        let file = CheatFile::parse(PathBuf::from("game.cht"), FILE);
        let entries = file.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "Master Code");
        assert_eq!(
            entries[0].cheat.as_ref().unwrap().format(),
            Format::ActionReplay
        );
        assert!(entries[1].enabled);
        assert_eq!(
            entries[1].cheat.as_ref().unwrap().format(),
            Format::CodeBreaker
        );
        assert_eq!(entries[2].cheat.as_ref().unwrap().format(), Format::Raw);
        assert_eq!(file.active(), 1);
    }

    #[test]
    fn toggling_rewrites_only_the_enable_lines() {
        let dir = std::env::temp_dir().join(format!("tuiba-cheats-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("game.gba");
        fs::write(CheatFile::path_for(&rom), FILE).unwrap();

        let mut file = CheatFile::load(&rom).unwrap();
        file.toggle(0).unwrap();
        // The raw cheat has no enable line yet: one is added under it.
        file.toggle(2).unwrap();
        let text = fs::read_to_string(file.path()).unwrap();
        let expected = FILE
            .replace("cheat0_enable = false", "cheat0_enable = true")
            .replace(
                "02000002:3456\"\n",
                "02000002:3456\"\ncheat2_enable = true\n",
            );
        assert_eq!(text, expected);

        let file = CheatFile::load(&rom).unwrap();
        assert!(file.entries().iter().all(|e| e.enabled));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        let file = CheatFile::load(Path::new("/nonexistent/tuiba/game.gba")).unwrap();
        assert!(file.entries().is_empty());
        assert_eq!(file.path(), Path::new("/nonexistent/tuiba/game.cht"));
    }

    #[test]
    fn broken_codes_say_why() {
        let file = CheatFile::parse(
            PathBuf::from("x.cht"),
            "cheat0_desc = \"bad\"\ncheat0_code = \"12345\"\ncheat0_enable = true\n",
        );
        let entry = &file.entries()[0];
        assert!(entry.cheat.as_ref().unwrap_err().contains("not a code"));
        assert_eq!(file.active(), 0, "a broken cheat is never active");
    }

    #[test]
    fn the_panel_moves_and_toggles() {
        let press = |code| KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        let mut panel = CheatsPanel::open();
        assert_eq!(panel.handle(press(KeyCode::Down), 3), None);
        assert_eq!(panel.handle(press(KeyCode::End), 3), None);
        assert_eq!(panel.handle(press(KeyCode::Down), 3), None);
        assert_eq!(
            panel.handle(press(KeyCode::Enter), 3),
            Some(Action::Toggle(2))
        );
        assert_eq!(
            panel.handle(press(KeyCode::Char(' ')), 0),
            None,
            "nothing to toggle"
        );
        assert_eq!(panel.handle(press(KeyCode::Esc), 3), Some(Action::Close));
    }
}
