//! The save-state panel: four slots, each with the frame it was taken
//! on, over a paused game.
//!
//! The panel owns what it shows. Opening it reads every slot from disk
//! once — a state is small and there are four of them — so moving
//! between slots, and loading one, costs nothing afterwards. What the
//! panel hands back to the caller is an [`Action`]; it does not touch
//! the running machine itself.

use std::path::Path;
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use tuiba_core::{Cartridge, Snapshot};

use crate::savestate::{self, SLOTS};
use crate::screen::GbaScreen;
use crate::theme;

/// What the panel wants the caller to do, once a key has been handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Put the selected slot's state back into the machine.
    Load,
    /// Freeze the machine into the selected slot.
    Save,
    /// Throw the selected slot away.
    Delete,
    /// Close the panel and carry on playing.
    Close,
}

/// One slot as the panel knows it.
struct Slot {
    /// The state, once it has been read. `None` for an empty slot, and
    /// also for a file that would not load — `problem` says which.
    snapshot: Option<Snapshot>,
    written_at: Option<SystemTime>,
    /// Why this slot cannot be used, if it cannot.
    problem: Option<String>,
}

impl Slot {
    /// Reads one slot, turning every failure into something the panel
    /// can show rather than something the caller has to handle.
    fn read(rom: &Path, cartridge: &Cartridge, number: usize) -> Self {
        let Some(path) = savestate::slot_path(rom, cartridge, number) else {
            return Self {
                snapshot: None,
                written_at: None,
                problem: Some("no state directory".to_string()),
            };
        };
        let written_at = savestate::written_at(&path);
        match savestate::read(&path, cartridge) {
            Ok(snapshot) => Self {
                snapshot,
                written_at,
                problem: None,
            },
            Err(problem) => Self {
                snapshot: None,
                written_at,
                problem: Some(problem),
            },
        }
    }

    const fn is_empty(&self) -> bool {
        self.snapshot.is_none() && self.problem.is_none()
    }
}

/// The panel over a paused game.
pub struct StatesPanel {
    slots: Vec<Slot>,
    selected: usize,
}

impl StatesPanel {
    /// Opens the panel on `rom`'s slots.
    #[must_use]
    pub fn open(rom: &Path, cartridge: &Cartridge, selected: usize) -> Self {
        Self {
            slots: (1..=SLOTS).map(|n| Slot::read(rom, cartridge, n)).collect(),
            selected: selected.min(SLOTS - 1),
        }
    }

    /// Which slot the cursor is on, counting from zero.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// The state in the selected slot, if it holds one.
    #[must_use]
    pub fn selected_snapshot(&self) -> Option<&Snapshot> {
        self.slots[self.selected].snapshot.as_ref()
    }

    /// Takes in what the caller just wrote or deleted, so the panel
    /// shows the change without reading the disk again.
    pub fn replace_selected(&mut self, snapshot: Option<Snapshot>) {
        let slot = &mut self.slots[self.selected];
        slot.written_at = snapshot.as_ref().map(|_| SystemTime::now());
        slot.snapshot = snapshot;
        slot.problem = None;
    }

    /// Moves the cursor and turns the action keys into an [`Action`].
    /// Keys the panel does not use are swallowed: a game must not see
    /// the keyboard while this is up.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.step(1),
            // The slots are a 2×2 grid: up and down move by a row.
            KeyCode::Up | KeyCode::Char('k') => self.step(-2),
            KeyCode::Down | KeyCode::Char('j') => self.step(2),
            KeyCode::Char(c @ '1'..='4') => {
                self.selected = c as usize - '1' as usize;
            }
            KeyCode::Enter => return Some(Action::Load),
            KeyCode::Char('s') => return Some(Action::Save),
            KeyCode::Char('x') | KeyCode::Delete => return Some(Action::Delete),
            KeyCode::Esc => return Some(Action::Close),
            _ => {}
        }
        None
    }

    /// Moves the cursor by `delta` slots, staying inside the grid.
    fn step(&mut self, delta: isize) {
        let target = self.selected as isize + delta;
        if (0..SLOTS as isize).contains(&target) {
            self.selected = target as usize;
        }
    }

    /// Draws the panel centred in `area`.
    ///
    /// The tiles take what the terminal has, up to [`TILE_WIDTH`] ×
    /// [`TILE_HEIGHT`]; below the size where a thumbnail says anything
    /// the panel falls back to a plain list, which still works in a
    /// window too small to play in.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let width = (TILE_WIDTH * 2 + TILE_GAP + 4)
            .max(FOOTER_WIDTH)
            .min(area.width);
        // Two borders, a padding column either side, and the footer row.
        let tile_width = (width.saturating_sub(4 + TILE_GAP) / 2).min(TILE_WIDTH);
        let tile_height =
            (area.height.min(TILE_HEIGHT * 2 + 3).saturating_sub(3) / 2).min(TILE_HEIGHT);
        let as_list = tile_width < MIN_TILE_WIDTH || tile_height < MIN_TILE_HEIGHT;
        let height = if as_list {
            SLOTS as u16 + 3
        } else {
            tile_height * 2 + 3
        }
        .min(area.height);
        let [popup] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(popup);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(Line::styled(" SAVE STATES ", theme::accent().bold()))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border())
            .padding(Padding::horizontal(1))
            .style(theme::text().bg(theme::SURFACE));
        let inside = block.inner(popup);
        frame.render_widget(block, popup);

        let [grid, footer] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inside);
        if as_list {
            self.draw_as_list(frame, grid);
        } else {
            self.draw_grid(frame, grid, tile_width, tile_height);
        }
        frame.render_widget(
            Paragraph::new(self.footer_hints()).alignment(Alignment::Center),
            footer,
        );
    }

    /// The four tiles, two by two.
    fn draw_grid(&self, frame: &mut Frame, grid: Rect, tile_width: u16, tile_height: u16) {
        let rows = Layout::vertical([Constraint::Length(tile_height); 2])
            .flex(Flex::Center)
            .split(grid);
        for (row, &row_area) in rows.iter().enumerate() {
            let columns = Layout::horizontal([
                Constraint::Length(tile_width),
                Constraint::Length(TILE_GAP),
                Constraint::Length(tile_width),
            ])
            .flex(Flex::Center)
            .split(row_area);
            for (column, tile) in [(0, columns[0]), (1, columns[2])] {
                self.draw_slot(frame, tile, row * 2 + column);
            }
        }
    }

    /// The same four slots where a thumbnail would not fit: one line
    /// each, cursor included.
    fn draw_as_list(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let focused = index == self.selected;
                let marker = if focused { "▸" } else { " " };
                let style = if focused { theme::text() } else { theme::dim() };
                Line::styled(format!("{marker} {}  {}", index + 1, describe(slot)), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// The keys that do something for the selected slot.
    fn footer_hints(&self) -> Line<'static> {
        let hints = if self.slots[self.selected].is_empty() {
            [("s", "save here"), ("esc", "close")].as_slice()
        } else {
            [
                ("⏎", "load"),
                ("s", "overwrite"),
                ("x", "delete"),
                ("esc", "close"),
            ]
            .as_slice()
        };
        let mut spans = Vec::new();
        for (key, label) in hints {
            spans.push(Span::styled(format!(" {key} "), theme::key()));
            spans.push(Span::styled(format!("{label}  "), theme::hint()));
        }
        Line::from(spans)
    }

    /// One tile: its frame (or why there is none) and when it was taken.
    fn draw_slot(&self, frame: &mut Frame, area: Rect, index: usize) {
        let slot = &self.slots[index];
        let focused = index == self.selected;
        let border = if focused {
            theme::accent()
        } else {
            theme::border()
        };
        let title = if focused {
            Line::styled(format!("▸ {} ", index + 1), theme::accent().bold())
        } else {
            Line::styled(format!(" {} ", index + 1), theme::dim())
        };
        let block = Block::default()
            .title(title)
            .title_bottom(Line::styled(
                format!(" {} ", describe(slot)),
                if focused { theme::text() } else { theme::dim() },
            ))
            .borders(Borders::ALL)
            .border_type(if focused {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(border);
        let inside = block.inner(area);
        frame.render_widget(block, area);

        if let Some(snapshot) = &slot.snapshot {
            frame.render_widget(GbaScreen::new(snapshot.framebuffer()), inside);
        } else {
            let (text, style) = match &slot.problem {
                Some(problem) => (problem.as_str(), Style::default().fg(theme::WARN)),
                None => ("empty", theme::dim()),
            };
            let [line] = Layout::vertical([Constraint::Length(1)])
                .flex(Flex::Center)
                .areas(inside);
            frame.render_widget(
                Paragraph::new(text)
                    .alignment(Alignment::Center)
                    .style(style),
                line,
            );
        }
    }
}

/// Largest tile, in cells. The frame inside is drawn at whatever scale
/// [`GbaScreen`] picks for the space it gets.
const TILE_WIDTH: u16 = 32;
const TILE_HEIGHT: u16 = 12;
/// Below this a thumbnail is a smudge rather than a memory, and the
/// panel shows a list instead.
const MIN_TILE_WIDTH: u16 = 16;
const MIN_TILE_HEIGHT: u16 = 6;
/// Space between the two columns.
const TILE_GAP: u16 = 3;
/// Enough for the longest footer.
const FOOTER_WIDTH: u16 = 46;

/// What a slot says under its tile.
fn describe(slot: &Slot) -> String {
    if slot.problem.is_some() {
        return "unreadable".to_string();
    }
    match slot.written_at {
        Some(at) => ago(at),
        None => "empty".to_string(),
    }
}

/// "just now", "4 minutes ago", "3 days ago" — relative, so no timezone
/// has to be guessed at to say something true.
pub fn ago(at: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(at) else {
        // A file from the future: a clock that moved, not worth a story.
        return "just now".to_string();
    };
    let secs = elapsed.as_secs();
    let (n, unit) = match secs {
        0..=45 => return "just now".to_string(),
        46..=5399 => ((secs + 30) / 60, "minute"),
        5400..=86_399 => ((secs + 1800) / 3600, "hour"),
        _ => ((secs + 43_200) / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn panel() -> StatesPanel {
        StatesPanel {
            slots: (0..SLOTS)
                .map(|_| Slot {
                    snapshot: None,
                    written_at: None,
                    problem: None,
                })
                .collect(),
            selected: 0,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn arrows_walk_the_grid_without_leaving_it() {
        let mut p = panel();
        assert_eq!(p.selected(), 0);
        p.handle(press(KeyCode::Left));
        assert_eq!(p.selected(), 0, "the first slot has nothing to its left");
        p.handle(press(KeyCode::Right));
        assert_eq!(p.selected(), 1);
        p.handle(press(KeyCode::Down));
        assert_eq!(p.selected(), 3);
        p.handle(press(KeyCode::Down));
        assert_eq!(p.selected(), 3, "the bottom row has nothing below it");
        p.handle(press(KeyCode::Up));
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn a_number_jumps_straight_to_its_slot() {
        let mut p = panel();
        p.handle(press(KeyCode::Char('3')));
        assert_eq!(p.selected(), 2);
    }

    #[test]
    fn the_action_keys_report_what_the_caller_should_do() {
        let mut p = panel();
        assert_eq!(p.handle(press(KeyCode::Enter)), Some(Action::Load));
        assert_eq!(p.handle(press(KeyCode::Char('s'))), Some(Action::Save));
        assert_eq!(p.handle(press(KeyCode::Char('x'))), Some(Action::Delete));
        assert_eq!(p.handle(press(KeyCode::Esc)), Some(Action::Close));
    }

    /// A game must not see the keyboard while the panel is up.
    #[test]
    fn other_keys_do_nothing_at_all() {
        let mut p = panel();
        for code in [KeyCode::Char('a'), KeyCode::Char(' '), KeyCode::Tab] {
            assert_eq!(p.handle(press(code)), None);
        }
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn elapsed_time_reads_as_english() {
        let now = SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - Duration::from_secs(60)), "1 minute ago");
        assert_eq!(ago(now - Duration::from_secs(4 * 60)), "4 minutes ago");
        assert_eq!(ago(now - Duration::from_secs(2 * 3600)), "2 hours ago");
        assert_eq!(ago(now - Duration::from_secs(3 * 86_400)), "3 days ago");
        assert_eq!(
            ago(now + Duration::from_secs(600)),
            "just now",
            "a file from the future should not tell a story"
        );
    }
}
