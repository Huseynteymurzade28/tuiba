//! The library screen: pick a cartridge, manage the folders it comes from.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use tuiba_core::Framebuffer;
use tuiba_core::memory::SaveType;

use crate::library::{Library, Recent, Rom, compact_home, expand_home, human_size};
use crate::screen::GbaScreen;
use crate::{preview, states, theme, wordmark};

/// What the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Start this cartridge.
    Play(PathBuf),
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
}

/// Height of the CARTRIDGE pane with no preview: the label strip and the
/// fact rows.
const DETAIL_HEIGHT: u16 = 10;
/// Width the facts keep before a preview may have what is left.
const FACTS_WIDTH: u16 = 24;
/// Columns between the facts and the preview.
const PREVIEW_GAP: u16 = 2;
/// Rows the folder list keeps whatever the cartridge pane wants.
const FOLDERS_MIN_HEIGHT: u16 = 6;
/// Scales a preview may be drawn at, roomiest first: 1/8 is 30×10 cells,
/// 1/12 is 20×7. Anything narrower than the smaller of those has no
/// preview at all.
const PREVIEW_SCALES: [u16; 2] = [8, 12];

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
    /// The selected cartridge's preview frame, kept until the selection
    /// moves off it. Read from the cache, never emulated here.
    preview: Option<(PathBuf, Framebuffer, SystemTime)>,
}

impl Picker {
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
            preview: None,
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
                        return Some(Outcome::Play(rom.path.clone()));
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
                    return Some(Outcome::Play(rom.path.clone()));
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

    /// Renders the screen.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(theme::text()), area);

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

        // The preview only earns its place where a whole frame fits
        // beside the facts, and where the folder list still has room.
        self.load_preview();
        let preview_size = self
            .selected_preview()
            .and_then(|_| Self::preview_size(side));
        let detail_height = preview_size.map_or(DETAIL_HEIGHT, |(_, rows)| {
            // Label strip, the frame, its caption, and the borders.
            3 + rows + 1 + 2
        });
        let [detail, folders] =
            Layout::vertical([Constraint::Length(detail_height), Constraint::Fill(1)]).areas(side);
        self.draw_detail(frame, detail, preview_size);
        self.draw_folders(frame, folders);
        self.draw_footer(frame, footer);
    }

    /// Reads the selected cartridge's preview, unless it is already the
    /// one in hand. A cartridge with no preview leaves `None`, and is
    /// not read again until the selection comes back to it.
    fn load_preview(&mut self) {
        let Some(path) = self
            .visible
            .get(self.rom_index)
            .and_then(|&i| self.roms.get(i))
            .map(|rom| rom.path.clone())
        else {
            self.preview = None;
            return;
        };
        if self.preview.as_ref().is_some_and(|(p, ..)| *p == path) {
            return;
        }
        self.preview = preview::read(&path).map(|(frame, at)| (path, frame, at));
    }

    /// The preview of the cartridge under the cursor, if that is still
    /// what `load_preview` last read.
    fn selected_preview(&self) -> Option<(&Framebuffer, SystemTime)> {
        let (path, frame, at) = self.preview.as_ref()?;
        let selected = self
            .visible
            .get(self.rom_index)
            .and_then(|&i| self.roms.get(i))?;
        (selected.path == *path).then_some((frame, *at))
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let [logo_area, right] = Layout::horizontal([
            Constraint::Length(wordmark::width() + 3),
            Constraint::Fill(1),
        ])
        .areas(area);
        if logo_area.width > wordmark::width() {
            let shades = [
                theme::accent().fg(theme::ACCENT_LIGHT),
                theme::accent().fg(theme::ACCENT_LIGHT),
                theme::accent(),
                theme::accent(),
            ];
            frame.render_widget(
                Paragraph::new(wordmark::lines(&shades))
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

    /// A thin separator under the header.
    fn draw_rule(frame: &mut Frame, area: Rect) {
        let width = usize::from(area.width);
        let rule = Line::from(vec![
            Span::styled(" ", theme::dim()),
            Span::styled("─".repeat(width.saturating_sub(2)), theme::border()),
        ]);
        frame.render_widget(Paragraph::new(vec![Line::default(), rule]), area);
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
                        theme::selected_unfocused().fg(theme::OK),
                    )
                } else {
                    (theme::text(), theme::dim(), theme::text().fg(theme::OK))
                };
                let marker = if selected { "▸ " } else { "  " };
                // On the accent-coloured selected row the accent highlight
                // would vanish; underline there instead.
                let hit_style = if selected && focused {
                    name_style.underlined()
                } else {
                    name_style.fg(theme::ACCENT_LIGHT).bold()
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
        let meta_style = Style::default().fg(theme::DIM).bg(theme::SURFACE);
        let meta = Line::from(Span::styled(
            format!(" {}", facts.join("  ·  ")),
            meta_style,
        ));
        let [name_row, meta_row] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(Paragraph::new(name).style(theme::selected()), name_row);
        frame.render_widget(Paragraph::new(meta).style(meta_style), meta_row);
    }

    /// The `label  value` rows under the cartridge label: what the file
    /// is, what the header says, how it saves and when it was played.
    fn cartridge_facts(
        rom: &Rom,
        save_type: Option<SaveType>,
        played: Option<usize>,
        width: u16,
    ) -> Vec<Line<'static>> {
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
                        Span::styled("ok", theme::text().fg(theme::OK))
                    } else {
                        Span::styled("bad checksum (homebrew?)", theme::text().fg(theme::WARN))
                    },
                ));
            }
            None => lines.push(row(
                "Header",
                Span::styled("missing — file too small", theme::text().fg(theme::WARN)),
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
                    Span::styled("  ·  ● saved", theme::text().fg(theme::OK))
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
        let path_width = usize::from(width).saturating_sub(8);
        lines.push(row(
            "Path",
            Span::styled(
                truncate_start(&compact_home(&rom.path), path_width),
                theme::dim(),
            ),
        ));
        lines
    }

    /// The biggest preview the side column can hold, in cells, or `None`
    /// where even the smallest would crowd out the facts or the folders.
    fn preview_size(side: Rect) -> Option<(u16, u16)> {
        PREVIEW_SCALES.into_iter().find_map(|scale| {
            let (cols, rows) = GbaScreen::thumbnail_size(scale);
            let fits_across = side.width >= FACTS_WIDTH + PREVIEW_GAP + cols + 2;
            let fits_down = side.height >= 3 + rows + 1 + 2 + FOLDERS_MIN_HEIGHT;
            (fits_across && fits_down).then_some((cols, rows))
        })
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect, preview_size: Option<(u16, u16)>) {
        let mut preview_area = None;
        let block = Self::pane("CARTRIDGE", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(rom) = self
            .visible
            .get(self.rom_index)
            .and_then(|&i| self.roms.get_mut(i))
        else {
            frame.render_widget(
                Paragraph::new("Select a cartridge to see its details.").style(theme::dim()),
                inner,
            );
            return;
        };
        let save_type = rom.save_type();
        let played = self.recent.rank(&rom.path);

        let [label_area, rows_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(inner);
        Self::draw_cartridge_label(frame, label_area, rom);
        let rows_area = if let Some((cols, _)) = preview_size {
            let [facts, _, shot] = Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(PREVIEW_GAP),
                Constraint::Length(cols),
            ])
            .areas(rows_area);
            preview_area = Some(shot);
            facts
        } else {
            rows_area
        };

        let lines = Self::cartridge_facts(rom, save_type, played, rows_area.width);
        frame.render_widget(Paragraph::new(lines), rows_area);
        if let Some(area) = preview_area {
            self.draw_preview(frame, area);
        }
    }

    /// The frame this cartridge was last left on, with its age under it.
    fn draw_preview(&self, frame: &mut Frame, area: Rect) {
        let Some((shot, taken_at)) = self.selected_preview() else {
            return;
        };
        let [image, caption] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(GbaScreen::thumbnail(shot), image);
        frame.render_widget(
            Paragraph::new(Line::styled(states::ago(taken_at), theme::dim()))
                .alignment(Alignment::Center),
            caption,
        );
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
                    style.fg(theme::WARN)
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
                    Style::default().fg(theme::TEXT).bg(theme::SURFACE),
                ),
                Span::styled("█", Style::default().fg(theme::ACCENT).bg(theme::SURFACE)),
                Span::styled("   ⏎ confirm  esc cancel", theme::hint()),
            ]),
            Mode::Filter => Line::from(vec![
                Span::styled(" Filter ", theme::key()),
                Span::styled(
                    format!(" {}", self.filter),
                    Style::default().fg(theme::TEXT).bg(theme::SURFACE),
                ),
                Span::styled("█", Style::default().fg(theme::ACCENT).bg(theme::SURFACE)),
                Span::styled("   ⏎ keep  esc clear  ↑↓ select", theme::hint()),
            ]),
            Mode::Browse => {
                if let Some((message, ok)) = &self.notice {
                    let color = if *ok { theme::OK } else { theme::WARN };
                    Line::from(Span::styled(
                        format!(" {message}"),
                        Style::default().fg(color).bg(theme::SURFACE),
                    ))
                } else {
                    let mut spans = Vec::new();
                    let hints: &[(&str, &str)] = match self.focus {
                        Focus::Roms => &[
                            ("↑↓", "select"),
                            ("⏎", "play"),
                            ("/", "filter"),
                            ("s", "sort"),
                            ("tab", "folders"),
                            ("a", "add folder"),
                            ("r", "rescan"),
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
                    for (key, label) in hints {
                        spans.push(Span::styled(format!(" {key} "), theme::key()));
                        spans.push(Span::styled(format!("{label}  "), theme::hint()));
                    }
                    Line::from(spans)
                }
            }
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme::SURFACE)),
            area,
        );
        // Version, tucked into the right corner.
        let version = format!("tuiba {} ", env!("CARGO_PKG_VERSION"));
        let width = version.chars().count() as u16;
        if area.width > width + 40 {
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
    use super::*;

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
            Some(Outcome::Play(PathBuf::from("/r/Pliko.gba")))
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
            Some(Outcome::Play(PathBuf::from("/r/b.gba")))
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
        assert!(p.library.folders.is_empty());

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
