//! The library screen: pick a cartridge, manage the folders it comes from.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use tuiba_core::Framebuffer;
use tuiba_core::memory::SaveType;

use crate::gamepad::{PadButton, PadStyle};
use crate::library::{Library, Location, Recent, Rom, compact_home, expand_home, human_size};
use crate::screen::{CELL_HEIGHT, CELL_WIDTH, GbaScreen};
use crate::theme::Preset;
use crate::{cover, theme, wordmark};

/// What the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Start this cartridge.
    Play(Location),
    /// Leave the program.
    Quit,
}

/// Which pane takes navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Roms,
    Folders,
}

/// Whether the footer shows key hints or a text prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Browse,
    AddFolder(String),
    /// Typing a filter; the list narrows as it grows.
    Filter,
    /// "Quit?" prompt: `y` or Enter confirms, anything else cancels.
    ConfirmQuit,
    /// Choosing a colour theme: the cursor previews, Enter keeps it and
    /// Esc goes back to `original`.
    Themes {
        index: usize,
        original: Preset,
    },
}

/// Orderings for the cartridge list, cycled with `s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Title,
    FileName,
    LastPlayed,
    Size,
}

impl Sort {
    const fn next(self) -> Self {
        match self {
            Self::Title => Self::FileName,
            Self::FileName => Self::LastPlayed,
            Self::LastPlayed => Self::Size,
            Self::Size => Self::Title,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::FileName => "file name",
            Self::LastPlayed => "last played",
            Self::Size => "size",
        }
    }
}

/// Screen state.
#[derive(Debug)]
pub struct Picker {
    library: Library,
    recent: Recent,
    /// Every cartridge found, in scan order.
    roms: Vec<Rom>,
    /// Indices into `roms` after filtering and sorting: what the list
    /// shows.
    visible: Vec<usize>,
    /// Position in `visible`.
    rom_index: usize,
    folder_index: usize,
    focus: Focus,
    mode: Mode,
    sort: Sort,
    /// Current filter text (matched case-insensitively against title,
    /// file name and game code).
    filter: String,
    /// Transient feedback shown in the footer until the next key.
    notice: Option<(String, bool)>,
    /// The connected pad's labels, for the footer hints.
    pad: Option<PadStyle>,
    /// When the screen was made: the clock the wordmark's shine runs on.
    born: Instant,
    /// Previews read so far, by shown path; `None` for a game never left
    /// on a frame. Dropped on rescan, which follows every game.
    covers: HashMap<PathBuf, Option<Framebuffer>>,
}

/// How often a shine sweeps across the wordmark, and how long it takes.
const SHINE_PERIOD: Duration = Duration::from_secs(7);
const SHINE_SWEEP: Duration = Duration::from_millis(1100);
/// Half the width of the shine band, in pixels.
const SHINE_HALF_WIDTH: f32 = 3.0;

impl Picker {
    /// The pad the footer is naming buttons for.
    #[must_use]
    pub const fn pad(&self) -> Option<PadStyle> {
        self.pad
    }

    /// Names `pad`'s buttons in the footer, or keys only with `None`.
    pub const fn set_pad(&mut self, pad: Option<PadStyle>) {
        self.pad = pad;
    }

    /// Scans `library` and shows it.
    #[must_use]
    pub fn new(library: Library) -> Self {
        Self::with_recent(library, Recent::load())
    }

    fn with_recent(library: Library, recent: Recent) -> Self {
        let mut picker = Self {
            library,
            recent,
            roms: Vec::new(),
            visible: Vec::new(),
            rom_index: 0,
            folder_index: 0,
            focus: Focus::Roms,
            mode: Mode::Browse,
            sort: Sort::Title,
            filter: String::new(),
            notice: None,
            pad: None,
            born: Instant::now(),
            covers: HashMap::new(),
        };
        picker.rescan();
        // Start on whatever was played last.
        if let Some(last) = picker.recent.last().map(Path::to_path_buf) {
            picker.select_path(&last);
        }
        if picker.library.folders.is_empty() {
            picker.focus = Focus::Folders;
        }
        picker
    }

    /// Re-reads every folder, keeping the selection on the same file
    /// when it still exists.
    pub fn rescan(&mut self) {
        let keep = self.selected().map(|r| r.path.clone());
        self.covers.clear();
        self.roms = self.library.scan();
        self.refresh_view();
        if let Some(path) = keep {
            self.select_path(&path);
        }
        self.folder_index = self
            .folder_index
            .min(self.library.folders.len().saturating_sub(1));
    }

    /// Records that `path` is being played, so it sorts first under
    /// "last played" and is preselected next time.
    pub fn mark_played(&mut self, path: &Path) {
        self.recent.push(path);
        if let Err(err) = self.recent.save() {
            self.notify(format!("could not save recent list: {err}"), false);
        }
        self.refresh_view();
        self.select_path(path);
    }

    /// The cartridge under the cursor.
    fn selected(&self) -> Option<&Rom> {
        self.visible
            .get(self.rom_index)
            .and_then(|&i| self.roms.get(i))
    }

    /// Moves the cursor to `path` if it is in the list.
    fn select_path(&mut self, path: &Path) {
        if let Some(pos) = self.visible.iter().position(|&i| self.roms[i].path == path) {
            self.rom_index = pos;
        }
    }

    /// Rebuilds `visible` from the filter and sort, keeping the cursor
    /// on the same cartridge when it survives.
    fn refresh_view(&mut self) {
        let keep = self.selected().map(|r| r.path.clone());
        let needle = self.filter.trim().to_lowercase();
        self.visible = (0..self.roms.len())
            .filter(|&i| needle.is_empty() || Self::matches(&self.roms[i], &needle))
            .collect();
        let recent = &self.recent;
        let roms = &self.roms;
        match self.sort {
            Sort::Title => self
                .visible
                .sort_by_cached_key(|&i| (roms[i].name().to_lowercase(), roms[i].path.clone())),
            Sort::FileName => self.visible.sort_by_cached_key(|&i| {
                (roms[i].file_name().to_lowercase(), roms[i].path.clone())
            }),
            // Never played sorts after everything played, then by title.
            Sort::LastPlayed => self.visible.sort_by_cached_key(|&i| {
                (
                    recent.rank(&roms[i].path).unwrap_or(usize::MAX),
                    roms[i].name().to_lowercase(),
                )
            }),
            // Largest first.
            Sort::Size => self.visible.sort_by_cached_key(|&i| {
                (
                    std::cmp::Reverse(roms[i].size),
                    roms[i].name().to_lowercase(),
                )
            }),
        }
        self.rom_index = 0;
        if let Some(path) = keep {
            self.select_path(&path);
        }
    }

    /// Whether `rom` matches the lower-cased filter `needle`.
    fn matches(rom: &Rom, needle: &str) -> bool {
        rom.name().to_lowercase().contains(needle)
            || rom.file_name().to_lowercase().contains(needle)
            || rom
                .game_code()
                .is_some_and(|code| code.to_lowercase().contains(needle))
    }

    /// Adds `folder` to the library, persists it and rescans.
    pub fn add_folder(&mut self, folder: &Path) {
        if !folder.is_dir() {
            self.notify(format!("not a folder: {}", folder.display()), false);
            return;
        }
        if !self.library.add(folder.to_path_buf()) {
            self.notify("already in the library", false);
            return;
        }
        match self.library.save() {
            Ok(()) => self.notify(format!("added {}", compact_home(folder)), true),
            Err(err) => self.notify(format!("could not save library: {err}"), false),
        }
        self.rescan();
        self.folder_index = self.library.folders.len() - 1;
        if !self.visible.is_empty() {
            self.focus = Focus::Roms;
        }
    }

    fn remove_folder(&mut self) {
        let Some(folder) = self.library.folders.get(self.folder_index).cloned() else {
            return;
        };
        self.library.remove(self.folder_index);
        if let Err(err) = self.library.save() {
            self.notify(format!("could not save library: {err}"), false);
        } else {
            self.notify(format!("removed {}", compact_home(&folder)), true);
        }
        self.rescan();
    }

    fn notify(&mut self, message: impl Into<String>, ok: bool) {
        self.notice = Some((message.into(), ok));
    }

    /// Shows an error from outside the screen (e.g. a ROM that failed to
    /// load) in the footer.
    pub fn notify_error(&mut self, message: impl Into<String>) {
        self.notify(message, false);
    }

    /// Handles a key; returns `Some` when the screen is done.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Outcome> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        self.notice = None;
        match &mut self.mode {
            Mode::Filter => return self.handle_filter(key),
            Mode::AddFolder(_) => {
                self.handle_add_folder(key);
                return None;
            }
            Mode::ConfirmQuit => {
                self.mode = Mode::Browse;
                return matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter)
                    .then_some(Outcome::Quit);
            }
            Mode::Themes { .. } => {
                self.handle_themes(key);
                return None;
            }
            Mode::Browse => {}
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // A held key never asks to quit: its repeats would otherwise carry
        // the Esc that left a game straight into the prompt.
        let pressed = key.kind == KeyEventKind::Press;
        match key.code {
            // Esc first drops an active filter, then asks to quit.
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refresh_view();
            }
            KeyCode::Esc | KeyCode::Char('q') if pressed => self.mode = Mode::ConfirmQuit,
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.focus = Focus::Roms;
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.refresh_view();
                self.notify(format!("sorted by {}", self.sort.label()), true);
            }
            KeyCode::Char('c') if ctrl && pressed => return Some(Outcome::Quit),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Roms => Focus::Folders,
                    Focus::Folders => Focus::Roms,
                };
            }
            KeyCode::Char('a') => self.mode = Mode::AddFolder(String::new()),
            KeyCode::Char('t') => {
                let original = theme::current();
                let index = Preset::ALL.iter().position(|&p| p == original).unwrap_or(0);
                self.mode = Mode::Themes { index, original };
            }
            KeyCode::Char('r') => {
                self.rescan();
                self.notify(format!("{} cartridges", self.roms.len()), true);
            }
            KeyCode::Char('x') | KeyCode::Delete if self.focus == Focus::Folders => {
                self.remove_folder();
            }
            KeyCode::Enter => match self.focus {
                Focus::Roms => {
                    if let Some(rom) = self.selected() {
                        return Some(Outcome::Play(rom.location()));
                    }
                }
                Focus::Folders => {
                    if self.library.folders.is_empty() {
                        self.mode = Mode::AddFolder(String::new());
                    } else if !self.visible.is_empty() {
                        self.focus = Focus::Roms;
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::End => self.move_by(isize::MAX / 2),
            _ => {}
        }
        None
    }

    /// Keys while typing a filter.
    fn handle_filter(&mut self, key: KeyEvent) -> Option<Outcome> {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Browse;
                self.refresh_view();
            }
            KeyCode::Enter => {
                self.mode = Mode::Browse;
                if let Some(rom) = self.selected()
                    && self.visible.len() == 1
                {
                    // One match: Enter plays it straight away.
                    return Some(Outcome::Play(rom.location()));
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.refresh_view();
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.clear();
                self.refresh_view();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(c);
                self.refresh_view();
            }
            _ => {}
        }
        None
    }

    /// Keys while the theme list is up.
    fn handle_themes(&mut self, key: KeyEvent) {
        let Mode::Themes { index, original } = &mut self.mode else {
            return;
        };
        let last = Preset::ALL.len() - 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *index = (*index + 1).min(last),
            KeyCode::Home => *index = 0,
            KeyCode::End => *index = last,
            KeyCode::Enter => {
                let preset = Preset::ALL[*index];
                self.mode = Mode::Browse;
                match theme::choose(preset) {
                    Ok(()) => self.notify(format!("theme: {}", preset.name()), true),
                    Err(err) => self.notify(
                        format!("theme: {} (not saved: {err})", preset.name()),
                        false,
                    ),
                }
                return;
            }
            KeyCode::Esc | KeyCode::Char('q' | 't') => {
                theme::preview(*original);
                self.mode = Mode::Browse;
                return;
            }
            _ => return,
        }
        theme::preview(Preset::ALL[*index]);
    }

    /// Keys while typing a folder path.
    fn handle_add_folder(&mut self, key: KeyEvent) {
        let Mode::AddFolder(input) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Browse,
            KeyCode::Enter => {
                let path = expand_home(input.trim());
                self.mode = Mode::Browse;
                if !path.as_os_str().is_empty() {
                    self.add_folder(&path);
                }
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.push(c);
            }
            _ => {}
        }
    }

    fn move_by(&mut self, delta: isize) {
        let (index, len) = match self.focus {
            Focus::Roms => (&mut self.rom_index, self.visible.len()),
            Focus::Folders => (&mut self.folder_index, self.library.folders.len()),
        };
        if len == 0 {
            return;
        }
        let target = (*index as isize).saturating_add(delta);
        *index = target.clamp(0, len as isize - 1) as usize;
    }

    /// Where the shine is, as a fraction of its sweep, if one is under
    /// way at `now`.
    fn shine(&self, now: Instant) -> Option<f32> {
        let period = SHINE_PERIOD.as_secs_f32();
        let t = (now - self.born).as_secs_f32() % period;
        // The first sweep comes a moment after the screen opens.
        let t = (t - 1.5).rem_euclid(period);
        (t < SHINE_SWEEP.as_secs_f32()).then(|| t / SHINE_SWEEP.as_secs_f32())
    }

    /// Whether the screen is moving and wants redrawing even without
    /// input. Pass `drawn_moving`, what this said at the last draw, so
    /// the frame after a sweep ends clears it.
    #[must_use]
    pub fn animating(&self, drawn_moving: bool) -> bool {
        drawn_moving || self.shine(Instant::now()).is_some()
    }

    /// Renders the screen.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(theme::page()), area);

        // Breathing room: a row above the header, a column either side.
        let [_, content, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let content = content.inner(Margin::new(1, 0));
        let [header, rule, body] = Layout::vertical([
            Constraint::Length(wordmark::HEIGHT),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(content);
        self.draw_header(frame, header);
        Self::draw_rule(frame, rule);

        let [list_area, side] =
            Layout::horizontal([Constraint::Percentage(56), Constraint::Fill(1)]).areas(body);
        self.draw_roms(frame, list_area);
        // The folder list takes what it needs; the preview the rest.
        let folder_rows = self.library.folders.len().clamp(1, 5) as u16 + 2;
        let [detail, folders] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(folder_rows)]).areas(side);
        self.draw_detail(frame, detail);
        self.draw_folders(frame, folders);
        self.draw_footer(frame, footer);
        if let Mode::Themes { index, .. } = self.mode {
            draw_themes(frame, area, index);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let [logo_area, right] = Layout::horizontal([
            Constraint::Length(wordmark::width() + 3),
            Constraint::Fill(1),
        ])
        .areas(area);
        if logo_area.width > wordmark::width() {
            let p = theme::palette();
            let shine = self.shine(Instant::now());
            let span = f32::from(wordmark::width()) + 2.0 * SHINE_HALF_WIDTH;
            // Light falls from the top left; the shine is a slanted band
            // of white sweeping left to right.
            let paint = |x: u16, y: u16| {
                let (x, y) = (f32::from(x), f32::from(y));
                let base = theme::mix(p.accent_light, p.accent, f32::midpoint(x / 30.0, y / 7.0));
                let Some(progress) = shine else { return base };
                let centre = progress * span - SHINE_HALF_WIDTH + y * 0.5;
                let near = 1.0 - (x - centre).abs() / SHINE_HALF_WIDTH;
                if near > 0.0 {
                    theme::mix(base, Color::Rgb(0xFF, 0xFF, 0xFF), near * 0.8)
                } else {
                    base
                }
            };
            frame.render_widget(
                Paragraph::new(wordmark::painted(paint, shadow(&p)))
                    .block(Block::default().padding(Padding::left(1))),
                logo_area,
            );
        } else {
            // Narrow terminal: plain text beats a cropped wordmark.
            frame.render_widget(
                Paragraph::new(Line::styled(" TUIBA", theme::accent().bold())),
                logo_area,
            );
        }

        let summary = match (self.roms.len(), self.library.folders.len()) {
            (_, 0) => "no folders yet".to_string(),
            (0, f) => format!("no cartridges in {f} {}", plural(f, "folder")),
            (r, f) => format!(
                "{r} {}  ·  {f} {}",
                plural(r, "cartridge"),
                plural(f, "folder")
            ),
        };
        let lines = vec![
            Line::default(),
            Line::from(vec![
                Span::styled("GAME BOY ADVANCE", theme::text().bold()),
                Span::styled("  ·  in your terminal ", theme::dim()),
            ]),
            Line::styled(format!("{summary} "), theme::dim()),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Right), right);
    }

    /// A thin separator under the header, glowing in the accent at the
    /// left and fading into the page towards the right.
    fn draw_rule(frame: &mut Frame, area: Rect) {
        let p = theme::palette();
        let width = area.width.saturating_sub(2);
        let mut spans = vec![Span::raw(" ")];
        spans.extend((0..width).map(|i| {
            let t = f32::from(i) / f32::from(width.max(1));
            let colour = if t < 0.5 {
                theme::mix(p.accent, p.dim, t * 2.0)
            } else {
                theme::mix(p.dim, p.surface, (t - 0.5) * 2.0)
            };
            Span::styled("─", Style::default().fg(colour))
        }));
        frame.render_widget(
            Paragraph::new(vec![Line::default(), Line::from(spans)]),
            area,
        );
    }

    fn pane(title: &str, focused: bool) -> Block<'static> {
        let (border, title_style) = if focused {
            (theme::accent(), theme::accent().bold())
        } else {
            (theme::border(), theme::dim())
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(Line::styled(format!(" {title} "), title_style))
            .padding(Padding::horizontal(1))
    }

    fn draw_roms(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Roms;
        // The title carries the sort and, when narrowed, the filter.
        let mut title = format!("LIBRARY · by {}", self.sort.label());
        if !self.filter.is_empty() {
            let _ = write!(
                title,
                " · /{} ({} of {})",
                self.filter,
                self.visible.len(),
                self.roms.len()
            );
        }
        let block = Self::pane(&title, focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.visible.is_empty() {
            let text = if self.library.folders.is_empty() {
                "No folders yet.\n\nPress a to add the folder your .gba files live in."
            } else if !self.filter.is_empty() {
                "Nothing matches the filter.\n\nKeep typing, or press esc to clear it."
            } else {
                "No .gba files found in the library folders.\n\nPress r to rescan or a to add another folder."
            };
            frame.render_widget(
                Paragraph::new(text)
                    .style(theme::dim())
                    .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }

        // Name on the left; game code and a save marker pinned right.
        let name_width = usize::from(inner.width).saturating_sub(12);
        let needle = self.filter.trim().to_lowercase();
        let items: Vec<ListItem> = self
            .visible
            .iter()
            .enumerate()
            .map(|(i, &rom_i)| {
                let rom = &self.roms[rom_i];
                let selected = i == self.rom_index;
                let name = truncate(&rom.name(), name_width);
                let code = rom.game_code().map_or_else(|| "----".to_string(), pad4);
                let save = if rom.has_save { "●" } else { " " };
                let (name_style, meta_style, save_style) = if selected && focused {
                    (theme::selected(), theme::selected(), theme::selected())
                } else if selected {
                    (
                        theme::selected_unfocused(),
                        theme::selected_unfocused(),
                        theme::selected_unfocused().fg(theme::palette().ok),
                    )
                } else {
                    (
                        theme::text(),
                        theme::dim(),
                        theme::text().fg(theme::palette().ok),
                    )
                };
                let marker = if selected { "▸ " } else { "  " };
                // On the accent-coloured selected row the accent highlight
                // would vanish; underline there instead.
                let hit_style = if selected && focused {
                    name_style.underlined()
                } else {
                    name_style.fg(theme::palette().accent_light).bold()
                };
                let mut spans = vec![Span::styled(marker, name_style)];
                spans.extend(highlight(&name, &needle, name_style, hit_style));
                let pad = name_width.saturating_sub(name.chars().count());
                spans.push(Span::styled(" ".repeat(pad), name_style));
                spans.push(Span::styled(format!("  {code}  "), meta_style));
                spans.push(Span::styled(save.to_string(), save_style));
                spans.push(Span::styled(" ", name_style));
                ListItem::new(Line::from(spans))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.rom_index));
        frame.render_stateful_widget(List::new(items), inner, &mut state);
        if focused {
            let row = (self.rom_index - state.offset()) as u16;
            let bar = Rect {
                x: inner.x.saturating_sub(1),
                y: inner.y + row,
                width: inner.width + 2,
                height: 1,
            };
            glow(frame.buffer_mut(), bar);
        }

        if self.visible.len() > usize::from(inner.height) {
            self.draw_scrollbar(frame, area, inner.height);
        }
    }

    /// A scrollbar down the list's right border, for lists longer than
    /// the `rows` it shows.
    fn draw_scrollbar(&self, frame: &mut Frame, area: Rect, rows: u16) {
        let mut scroll = ScrollbarState::new(self.visible.len())
            .viewport_content_length(usize::from(rows))
            .position(self.rom_index);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(theme::accent())
                .thumb_symbol("┃")
                .thumb_style(theme::accent().fg(theme::palette().accent_light)),
            area.inner(Margin::new(0, 1)),
            &mut scroll,
        );
    }

    /// A label strip across the top of the detail pane, like the sticker
    /// on a cartridge: the name in the accent bar, then code, version and
    /// size in a quieter line beneath it.
    fn draw_cartridge_label(frame: &mut Frame, area: Rect, rom: &Rom) {
        let width = usize::from(area.width);
        let name = Line::from(Span::styled(
            format!(" {}", truncate(&rom.name(), width.saturating_sub(2))),
            theme::selected(),
        ));
        let mut facts = Vec::new();
        if let Some(h) = &rom.header {
            if let Some(game_code) = rom.game_code() {
                facts.push(game_code.to_string());
            }
            facts.push(format!("v1.{}", h.version));
        }
        facts.push(human_size(rom.size));
        let meta_style = Style::default()
            .fg(theme::palette().dim)
            .bg(theme::palette().surface);
        let meta = Line::from(Span::styled(
            format!(" {}", facts.join("  ·  ")),
            meta_style,
        ));
        let [name_row, meta_row] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(Paragraph::new(name).style(theme::selected()), name_row);
        frame.render_widget(Paragraph::new(meta).style(meta_style), meta_row);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect) {
        let block = Self::pane("CARTRIDGE", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(&rom_i) = self.visible.get(self.rom_index) else {
            frame.render_widget(
                Paragraph::new("Select a cartridge to see its details.").style(theme::dim()),
                inner,
            );
            return;
        };
        // The facts keep their rows; whatever is above them shows the game.
        let [preview_area, label_area, rows_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Length(6),
        ])
        .areas(inner);
        self.draw_preview(frame, preview_area, rom_i);
        let rom = &mut self.roms[rom_i];
        let save_type = rom.save_type();
        let played = self.recent.rank(&rom.path);
        Self::draw_cartridge_label(frame, label_area, rom);

        let row = |label: &str, value: Span<'static>| {
            Line::from(vec![
                Span::styled(format!("{label:<8}"), theme::dim()),
                value,
            ])
        };
        let row_spans = |label: &str, values: Vec<Span<'static>>| {
            let mut spans = vec![Span::styled(format!("{label:<8}"), theme::dim())];
            spans.extend(values);
            Line::from(spans)
        };
        let plain = |s: String| Span::styled(s, theme::text());

        let mut lines = vec![row("File", plain(format!("{}.gba", rom.file_name())))];
        match &rom.header {
            Some(h) => {
                lines.push(row(
                    "Maker",
                    plain(if h.maker_code.trim().is_empty() {
                        "unknown".to_string()
                    } else {
                        h.maker_code.clone()
                    }),
                ));
                lines.push(row(
                    "Header",
                    if h.valid {
                        Span::styled("ok", theme::text().fg(theme::palette().ok))
                    } else {
                        Span::styled(
                            "bad checksum (homebrew?)",
                            theme::text().fg(theme::palette().warn),
                        )
                    },
                ));
            }
            None => lines.push(row(
                "Header",
                Span::styled(
                    "missing — file too small",
                    theme::text().fg(theme::palette().warn),
                ),
            )),
        }
        let chip = match save_type {
            Some(SaveType::Sram) => "SRAM 32K",
            Some(SaveType::Flash64K) => "Flash 64K",
            Some(SaveType::Flash128K) => "Flash 128K",
            Some(SaveType::Eeprom) => "EEPROM",
            Some(SaveType::Unknown) => "none",
            None => "unreadable",
        };
        lines.push(row_spans(
            "Save",
            vec![
                plain(chip.to_string()),
                if rom.has_save {
                    Span::styled("  ·  ● saved", theme::text().fg(theme::palette().ok))
                } else {
                    Span::styled("  ·  no save", theme::dim())
                },
            ],
        ));
        lines.push(row(
            "Played",
            match played {
                Some(0) => Span::styled("most recent", theme::text()),
                Some(n) => plain(format!("{n} games ago")),
                None => Span::styled("never", theme::dim()),
            },
        ));
        let path_width = usize::from(inner.width).saturating_sub(8);
        lines.push(row(
            "Path",
            Span::styled(
                truncate_start(&compact_home(&rom.location().shown_path()), path_width),
                theme::dim(),
            ),
        ));
        frame.render_widget(Paragraph::new(lines), rows_area);
    }

    /// The game as it was last left, in a little screen bezel, or a
    /// drawing of the cartridge when it has never been played.
    fn draw_preview(&mut self, frame: &mut Frame, area: Rect, rom_i: usize) {
        // A row of air between the picture and the label below it.
        let area = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        let room = Rect {
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
            ..area
        };
        if room.height < 6 || !GbaScreen::fits(room) {
            return;
        }
        let scale = GbaScreen::scale_for(room);
        let [bezel] = Layout::horizontal([Constraint::Length(CELL_WIDTH.div_ceil(scale) + 2)])
            .flex(Flex::Center)
            .areas(area);
        let [bezel] = Layout::vertical([Constraint::Length(CELL_HEIGHT.div_ceil(scale) + 2)])
            .flex(Flex::Center)
            .areas(bezel);

        let shown = self.roms[rom_i].location().shown_path();
        let cover = self
            .covers
            .entry(shown)
            .or_insert_with_key(|path| cover::load(path));
        let caption = if cover.is_some() {
            " last played frame "
        } else {
            " not played yet "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border())
            .title_bottom(Line::styled(caption, theme::dim()).right_aligned())
            .style(Style::default().bg(theme::palette().surface));
        let screen = block.inner(bezel);
        frame.render_widget(block, bezel);
        if let Some(cover) = cover {
            frame.render_widget(GbaScreen::new(cover), screen);
        } else {
            let name = self.roms[rom_i].name();
            draw_cartridge_art(frame.buffer_mut(), screen, &name);
        }
    }

    fn draw_folders(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Folders;
        let block = Self::pane("FOLDERS", focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.library.folders.is_empty() {
            frame.render_widget(
                Paragraph::new("empty — press a to add one").style(theme::dim()),
                inner,
            );
            return;
        }
        let width = usize::from(inner.width).saturating_sub(8);
        let items: Vec<ListItem> = self
            .library
            .folders
            .iter()
            .enumerate()
            .map(|(i, folder)| {
                let count = self.roms.iter().filter(|r| r.folder == i).count();
                let selected = i == self.folder_index;
                let style = if selected && focused {
                    theme::selected()
                } else if selected {
                    theme::selected_unfocused()
                } else {
                    theme::text()
                };
                let unreadable = !folder.is_dir();
                let label = truncate(&compact_home(folder), width);
                let count = if unreadable {
                    "  missing".to_string()
                } else {
                    format!("{count:>4}")
                };
                let count_style = if unreadable {
                    style.fg(theme::palette().warn)
                } else if selected && focused {
                    style
                } else {
                    theme::dim()
                };
                let marker = if selected { "▸ " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{marker}{label:<width$}"), style),
                    Span::styled(count, count_style),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.folder_index));
        frame.render_stateful_widget(List::new(items), inner, &mut state);
    }

    // This method intentionally keeps footer rendering modes together.
    #[allow(clippy::too_many_lines)]
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = match &self.mode {
            Mode::ConfirmQuit => Line::from(vec![
                Span::styled(" Quit tuiba? ", theme::key()),
                Span::styled("   y yes  esc / n no", theme::hint()),
            ]),
            Mode::AddFolder(input) => Line::from(vec![
                Span::styled(" Add folder ", theme::key()),
                Span::styled(
                    format!(" {input}"),
                    Style::default()
                        .fg(theme::palette().text)
                        .bg(theme::palette().surface),
                ),
                Span::styled(
                    "█",
                    Style::default()
                        .fg(theme::palette().accent)
                        .bg(theme::palette().surface),
                ),
                Span::styled("   ⏎ confirm  esc cancel", theme::hint()),
            ]),
            Mode::Filter => Line::from(vec![
                Span::styled(" Filter ", theme::key()),
                Span::styled(
                    format!(" {}", self.filter),
                    Style::default()
                        .fg(theme::palette().text)
                        .bg(theme::palette().surface),
                ),
                Span::styled(
                    "█",
                    Style::default()
                        .fg(theme::palette().accent)
                        .bg(theme::palette().surface),
                ),
                Span::styled("   ⏎ keep  esc clear  ↑↓ select", theme::hint()),
            ]),
            // The theme list carries its own hints.
            Mode::Browse | Mode::Themes { .. } => {
                if let Some((message, ok)) = &self.notice {
                    let color = if *ok {
                        theme::palette().ok
                    } else {
                        theme::palette().warn
                    };
                    Line::from(Span::styled(
                        format!(" {message}"),
                        Style::default().fg(color).bg(theme::palette().surface),
                    ))
                } else {
                    let mut spans = Vec::new();
                    // With a pad connected, its buttons lead the way;
                    // the keyboard hints follow for everything else.
                    let pad_hints = match (self.pad, self.focus) {
                        (Some(style), Focus::Roms) => vec![
                            ("↑↓".to_string(), "select"),
                            (style.label(style.confirm()).to_string(), "play"),
                            (
                                format!(
                                    "{}/{}",
                                    style.label(PadButton::L1),
                                    style.label(PadButton::R1)
                                ),
                                "page",
                            ),
                        ],
                        _ => Vec::new(),
                    };
                    for (key, label) in &pad_hints {
                        spans.push(Span::styled(format!(" {key} "), theme::key()));
                        spans.push(Span::styled(format!("{label}  "), theme::hint()));
                    }
                    let hints: &[(&str, &str)] = match self.focus {
                        Focus::Roms => &[
                            ("↑↓", "select"),
                            ("⏎", "play"),
                            ("/", "filter"),
                            ("s", "sort"),
                            ("tab", "folders"),
                            ("a", "add folder"),
                            ("r", "rescan"),
                            ("t", "theme"),
                            ("q", "quit"),
                        ],
                        Focus::Folders => &[
                            ("↑↓", "select"),
                            ("a", "add folder"),
                            ("x", "remove"),
                            ("tab", "library"),
                            ("r", "rescan"),
                            ("q", "quit"),
                        ],
                    };
                    let hints = hints
                        .iter()
                        .filter(|(key, _)| pad_hints.is_empty() || !matches!(*key, "↑↓" | "⏎"));
                    for (key, label) in hints {
                        spans.push(Span::styled(format!(" {key} "), theme::key()));
                        spans.push(Span::styled(format!("{label}  "), theme::hint()));
                    }
                    Line::from(spans)
                }
            }
        };
        let used = line.width() as u16;
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme::palette().surface)),
            area,
        );
        // Version, tucked into the right corner when the hints leave room.
        let version = format!("tuiba {} ", env!("CARGO_PKG_VERSION"));
        let width = version.chars().count() as u16;
        if area.width >= used + width + 2 {
            frame.render_widget(
                Paragraph::new(Line::styled(version, theme::hint())),
                Rect {
                    x: area.right() - width,
                    width,
                    ..area
                },
            );
        }
    }
}

/// A cartridge drawn in cells, centred in `area`: a grey shell with a
/// grip along the top and a label in the accent carrying `title`.
fn draw_cartridge_art(buf: &mut Buffer, area: Rect, title: &str) {
    let p = theme::palette();
    // Wider than tall, like the real thing; a cell is twice as tall as
    // it is wide.
    let width = (area.width * 3 / 5).clamp(18, 56).min(area.width);
    let height = area.height.saturating_sub(2).min(width * 5 / 16 + 1);
    if height < 6 {
        return;
    }
    let [cart] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [cart] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(cart);
    let shell = theme::mix(p.dim, p.surface, 0.35);
    let ridge = theme::mix(shell, p.surface, 0.5);
    let mut put = |x: u16, y: u16, symbol: &str, fg: Color, bg: Color| {
        if let Some(cell) = buf.cell_mut((cart.x + x, cart.y + y)) {
            cell.set_symbol(symbol).set_fg(fg).set_bg(bg);
        }
    };
    for y in 0..height {
        for x in 0..width {
            put(x, y, " ", shell, shell);
        }
    }
    // Rounded corners: half the corner cell is shell, half background.
    for x in [0, width - 1] {
        put(x, 0, "▄", shell, p.surface);
        put(x, height - 1, "▀", shell, p.surface);
    }
    // The grip ridges along the top edge.
    for x in 3..width - 3 {
        put(x, 1, "═", ridge, shell);
    }

    // The label, lit from above.
    let label = Rect {
        x: 2,
        y: 3,
        width: width - 4,
        height: height.saturating_sub(5).max(2),
    };
    for row in 0..label.height {
        let bg = theme::mix(
            p.accent_light,
            p.accent,
            f32::from(row) / f32::from(label.height.max(2) - 1),
        );
        for x in 0..label.width {
            put(label.x + x, label.y + row, " ", p.bg, bg);
        }
    }
    let text_width = usize::from(label.width.saturating_sub(2));
    let mut lines = wrap_words(title, text_width);
    lines.truncate(usize::from(label.height.saturating_sub(1)).max(1));
    let console = if text_width >= 16 {
        "GAME BOY ADVANCE"
    } else {
        "GBA"
    };
    let rows = lines.len() as u16;
    let top = label.y + (label.height.saturating_sub(1).saturating_sub(rows)) / 2;
    let mut write = |y: u16, text: &str, fg: Color, bold: bool| {
        let pad = (label.width - text.chars().count() as u16) / 2;
        for (i, ch) in text.chars().enumerate() {
            if let Some(cell) = buf.cell_mut((cart.x + label.x + pad + i as u16, cart.y + y)) {
                cell.set_char(ch).set_fg(fg);
                if bold {
                    cell.modifier.insert(ratatui::style::Modifier::BOLD);
                }
            }
        }
    };
    for (i, line) in lines.iter().enumerate() {
        write(top + i as u16, line, p.on_accent, true);
    }
    write(
        label.y + label.height - 1,
        console,
        theme::mix(p.accent, p.on_accent, 0.55),
        false,
    );
}

/// `text` broken into lines of at most `width` characters at spaces,
/// overlong words cut.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = truncate(word, width);
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(&word);
            }
            _ => lines.push(word),
        }
    }
    lines
}

/// The wordmark's drop shadow: the accent sunk into the page, or the
/// raised surface where the page is the terminal's own and cannot be
/// blended with.
fn shadow(p: &theme::Palette) -> Color {
    match p.bg {
        Color::Rgb(..) => theme::mix(p.accent, p.bg, 0.7),
        _ => p.surface,
    }
}

/// The theme list over the middle of the screen: each preset's name,
/// what it looks like and a strip of its colours. The screen behind
/// already wears the one under the cursor.
fn draw_themes(frame: &mut Frame, area: Rect, index: usize) {
    let rows = Preset::ALL.len() as u16;
    let [popup] = Layout::horizontal([Constraint::Length(62)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(rows + 5)])
        .flex(Flex::Center)
        .areas(popup);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(Line::styled(" THEMES ", theme::accent().bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .padding(Padding::new(1, 1, 1, 0))
        .style(theme::text().bg(theme::palette().surface));
    let inside = block.inner(popup);
    frame.render_widget(block, popup);
    let [list, _, footer] = Layout::vertical([
        Constraint::Length(rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inside);

    for (i, preset) in Preset::ALL.into_iter().enumerate() {
        let selected = i == index;
        let row = Rect {
            y: list.y + i as u16,
            height: 1,
            ..list
        };
        let (name_style, about_style) = if selected {
            (theme::selected(), theme::selected().not_bold())
        } else {
            (theme::text().bold(), theme::dim())
        };
        let marker = if selected { "▸ " } else { "  " };
        let text = Line::from(vec![
            Span::styled(format!("{marker}{:<10}", preset.name()), name_style),
            Span::styled(preset.description(), about_style),
        ]);
        frame.render_widget(Paragraph::new(text), row);
        if selected {
            glow(frame.buffer_mut(), row);
        }
        // Swatches pinned right, each in the preset's own colour.
        let c = preset.palette();
        let colours = [c.bg, c.surface, c.dim, c.accent, c.accent_light, c.text];
        let width = colours.len() as u16 * 2;
        let swatch_area = Rect {
            x: row.right().saturating_sub(width + 1),
            width,
            ..row
        };
        let swatches: Vec<Span> = colours
            .iter()
            .map(|&colour| Span::styled("██", Style::default().fg(colour)))
            .collect();
        frame.render_widget(Paragraph::new(Line::from(swatches)), swatch_area);
    }

    let hints = [("↑↓", "preview"), ("⏎", "keep"), ("esc", "cancel")];
    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(format!(" {key} "), theme::key()));
        spans.push(Span::styled(format!("{label}  "), theme::hint()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        footer,
    );
}

/// Recolours the background of `area`, a selected row, from the accent
/// at the left to a deeper shade at the right, keeping its text.
fn glow(buf: &mut Buffer, area: Rect) {
    let p = theme::palette();
    let end = theme::mix(p.accent, p.bg, 0.45);
    let last = f32::from(area.width.saturating_sub(1).max(1));
    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.bg = theme::mix(p.accent, end, f32::from(x - area.x) / last);
        }
    }
}

/// `word` or its plural, by count.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Cuts `s` to `width` characters, marking the cut with an ellipsis.
fn truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        s.to_string()
    } else if width == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(width - 1).collect();
        out.push('…');
        out
    }
}

/// Like [`truncate`], but keeps the end of the string.
fn truncate_start(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        s.to_string()
    } else if width == 0 {
        String::new()
    } else {
        let mut out = String::from('…');
        out.extend(s.chars().skip(count - (width - 1)));
        out
    }
}

/// `text` as spans, with the first case-insensitive occurrence of
/// `needle` in `hit` instead of `base`.
fn highlight(text: &str, needle: &str, base: Style, hit: Style) -> Vec<Span<'static>> {
    let found = if needle.is_empty() {
        None
    } else {
        text.to_lowercase().find(needle)
    };
    match found {
        // Byte offsets from the lower-cased copy may not line up with
        // the original for exotic case mappings; fall back to plain text.
        Some(start)
            if text.is_char_boundary(start) && text.is_char_boundary(start + needle.len()) =>
        {
            let end = start + needle.len();
            vec![
                Span::styled(text[..start].to_string(), base),
                Span::styled(text[start..end].to_string(), hit),
                Span::styled(text[end..].to_string(), base),
            ]
        }
        _ => vec![Span::styled(text.to_string(), base)],
    }
}

/// Pads or trims a game code to four columns.
fn pad4(code: &str) -> String {
    format!("{:<4}", truncate(code, 4))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::library::Source;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker_with(roms: Vec<Rom>, folders: Vec<PathBuf>) -> Picker {
        let mut picker = Picker::with_recent(Library::default(), Recent::default());
        picker.library.folders = folders;
        picker.roms = roms;
        picker.refresh_view();
        picker.focus = Focus::Roms;
        picker
    }

    fn names(p: &Picker) -> Vec<String> {
        p.visible.iter().map(|&i| p.roms[i].name()).collect()
    }

    #[test]
    fn filter_narrows_highlights_and_clears() {
        let mut p = picker_with(
            vec![rom("Anguna"), rom("Pliko"), rom("Heartwrench")],
            vec![PathBuf::from("/r")],
        );
        p.handle(key(KeyCode::Char('/')));
        for c in "LI".chars() {
            p.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(names(&p), ["Pliko"], "case-insensitive");
        assert_eq!(
            highlight("Pliko", "li", theme::text(), theme::accent())[1].content,
            "li"
        );
        // One match: Enter plays it.
        assert_eq!(
            p.handle(key(KeyCode::Enter)),
            Some(Outcome::Play(Location::file(PathBuf::from("/r/Pliko.gba"))))
        );
        // Esc in browse mode clears the filter before it asks to quit.
        assert_eq!(p.handle(key(KeyCode::Esc)), None);
        assert_eq!(names(&p).len(), 3);
        assert_eq!(p.handle(key(KeyCode::Esc)), None);
        assert_eq!(p.mode, Mode::ConfirmQuit);
        assert_eq!(p.handle(key(KeyCode::Char('y'))), Some(Outcome::Quit));
    }

    #[test]
    fn sort_cycles_and_last_played_leads() {
        let mut big = rom("big");
        big.size = 1 << 20;
        let mut p = picker_with(vec![rom("b"), big, rom("a")], vec![PathBuf::from("/r")]);
        assert_eq!(names(&p), ["a", "b", "big"]);
        p.handle(key(KeyCode::Char('s')));
        assert_eq!(p.sort, Sort::FileName);
        p.handle(key(KeyCode::Char('s')));
        assert_eq!(p.sort, Sort::LastPlayed);
        assert_eq!(names(&p), ["a", "b", "big"], "nothing played: by title");
        p.recent.push(Path::new("/r/big.gba"));
        p.refresh_view();
        assert_eq!(names(&p), ["big", "a", "b"]);
        p.handle(key(KeyCode::Char('s')));
        assert_eq!(p.sort, Sort::Size);
        assert_eq!(names(&p)[0], "big", "largest first");
        // Through all of that the cursor followed the cartridge, not the row.
        assert_eq!(p.selected().map(Rom::name), Some("a".into()));
        assert_eq!(p.rom_index, 1);
    }

    fn rom(name: &str) -> Rom {
        Rom {
            path: PathBuf::from(format!("/r/{name}.gba")),
            source: Source::File,
            folder: 0,
            header: None,
            size: 0,
            has_save: false,
            save_type: None,
        }
    }

    #[test]
    fn navigation_clamps_and_enter_plays() {
        let mut p = picker_with(
            vec![rom("a"), rom("b"), rom("c")],
            vec![PathBuf::from("/r")],
        );
        assert_eq!(p.handle(key(KeyCode::Up)), None);
        assert_eq!(p.rom_index, 0);
        p.handle(key(KeyCode::PageDown));
        assert_eq!(p.rom_index, 2);
        p.handle(key(KeyCode::Char('k')));
        assert_eq!(p.rom_index, 1);
        assert_eq!(
            p.handle(key(KeyCode::Enter)),
            Some(Outcome::Play(Location::file(PathBuf::from("/r/b.gba"))))
        );
        // q asks first; any key but y/Enter backs out.
        assert_eq!(p.handle(key(KeyCode::Char('q'))), None);
        assert_eq!(p.mode, Mode::ConfirmQuit);
        assert_eq!(p.handle(key(KeyCode::Char('n'))), None);
        assert_eq!(p.mode, Mode::Browse);
        p.handle(key(KeyCode::Esc));
        assert_eq!(p.handle(key(KeyCode::Esc)), None, "Esc cancels the prompt");
        assert_eq!(p.mode, Mode::Browse);
        p.handle(key(KeyCode::Char('q')));
        assert_eq!(p.handle(key(KeyCode::Enter)), Some(Outcome::Quit));
        let mut repeat = key(KeyCode::Esc);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(p.handle(repeat), None, "a held key does not ask");
        assert_eq!(p.mode, Mode::Browse);
    }

    #[test]
    fn add_folder_prompt_collects_text() {
        let mut p = picker_with(Vec::new(), Vec::new());
        p.handle(key(KeyCode::Char('a')));
        for c in "/definitely/missing".chars() {
            p.handle(key(KeyCode::Char(c)));
        }
        p.handle(key(KeyCode::Backspace));
        assert_eq!(p.mode, Mode::AddFolder("/definitely/missin".into()));
        assert_eq!(p.handle(key(KeyCode::Enter)), None);
        assert_eq!(p.mode, Mode::Browse);
        assert!(
            p.notice
                .as_ref()
                .is_some_and(|(m, ok)| !ok && m.starts_with("not a folder"))
        );
        assert_eq!(p.library.folders, Vec::<PathBuf>::new());

        // Esc abandons the prompt without quitting.
        p.handle(key(KeyCode::Char('a')));
        assert_eq!(p.handle(key(KeyCode::Esc)), None);
        assert_eq!(p.mode, Mode::Browse);
    }

    #[test]
    fn empty_library_starts_on_folders_and_enter_opens_the_prompt() {
        let mut p = Picker::new(Library::default());
        assert_eq!(p.focus, Focus::Folders);
        p.handle(key(KeyCode::Enter));
        assert_eq!(p.mode, Mode::AddFolder(String::new()));
    }

    #[test]
    fn truncation_marks_the_cut() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("x", 0), "");
        assert_eq!(truncate_start("hello world", 6), "…world");
        assert_eq!(truncate_start("hi", 6), "hi");
        assert_eq!(pad4("AB"), "AB  ");
    }

    fn render(picker: &mut Picker, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| picker.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_library_and_empty_state() {
        let dir = std::env::temp_dir().join(format!("tuiba-picker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rom = vec![0u8; 0x400];
        rom[0xA0..0xA6].copy_from_slice(b"TETRIS");
        rom[0xAC..0xB0].copy_from_slice(b"ATET");
        std::fs::write(dir.join("tetris.gba"), &rom).unwrap();
        std::fs::write(dir.join("tetris.sav"), [0; 8]).unwrap();

        let mut picker = Picker::new(Library {
            folders: vec![dir.clone()],
        });
        let screen = render(&mut picker, 90, 24);
        assert!(screen.contains("▸ TETRIS"), "{screen}");
        assert!(screen.contains("ATET  ●"), "save marker: {screen}");
        assert!(screen.contains(" TETRIS"), "label strip: {screen}");
        assert!(
            screen.contains("ATET  ·  v1.0  ·  1 KiB"),
            "label facts: {screen}"
        );
        assert!(screen.contains("● saved"), "{screen}");
        assert!(screen.contains("⏎ play"), "{screen}");

        picker.mode = Mode::AddFolder("~/ro".into());
        let screen = render(&mut picker, 90, 24);
        assert!(screen.contains("Add folder  ~/ro█"), "{screen}");

        let mut empty = Picker::new(Library::default());
        let screen = render(&mut empty, 60, 16);
        assert!(screen.contains("No folders yet."), "{screen}");
        assert!(screen.contains("x remove"), "{screen}");
        // Tiny terminals must not panic.
        render(&mut empty, 10, 4);
        render(&mut picker, 1, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
