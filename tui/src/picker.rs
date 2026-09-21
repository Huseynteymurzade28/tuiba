//! The library screen: pick a cartridge, manage the folders it comes from.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use tuiba_core::memory::SaveType;

use crate::library::{Library, Rom, compact_home, expand_home, human_size};
use crate::theme;

/// Two-row half-block wordmark.
const LOGO: [&str; 2] = ["▀█▀ █ █ █ █▀▄ ▄▀▄", " █  █▄█ █ █▄▀ █▀█"];

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
}

/// Screen state.
#[derive(Debug)]
pub struct Picker {
    library: Library,
    roms: Vec<Rom>,
    rom_index: usize,
    folder_index: usize,
    focus: Focus,
    mode: Mode,
    /// Transient feedback shown in the footer until the next key.
    notice: Option<(String, bool)>,
}

impl Picker {
    /// Scans `library` and shows it.
    #[must_use]
    pub fn new(library: Library) -> Self {
        let mut picker = Self {
            library,
            roms: Vec::new(),
            rom_index: 0,
            folder_index: 0,
            focus: Focus::Roms,
            mode: Mode::Browse,
            notice: None,
        };
        picker.rescan();
        if picker.library.folders.is_empty() {
            picker.focus = Focus::Folders;
        }
        picker
    }

    /// Re-reads every folder, keeping the selection on the same file
    /// when it still exists.
    pub fn rescan(&mut self) {
        let keep = self.roms.get(self.rom_index).map(|r| r.path.clone());
        self.roms = self.library.scan();
        self.rom_index = keep
            .and_then(|p| self.roms.iter().position(|r| r.path == p))
            .unwrap_or(0)
            .min(self.roms.len().saturating_sub(1));
        self.folder_index = self
            .folder_index
            .min(self.library.folders.len().saturating_sub(1));
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
        if !self.roms.is_empty() {
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
        if let Mode::AddFolder(input) = &mut self.mode {
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
            return None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Some(Outcome::Quit),
            KeyCode::Char('c') if ctrl => return Some(Outcome::Quit),
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
                    if let Some(rom) = self.roms.get(self.rom_index) {
                        return Some(Outcome::Play(rom.path.clone()));
                    }
                }
                Focus::Folders => {
                    if self.library.folders.is_empty() {
                        self.mode = Mode::AddFolder(String::new());
                    } else if !self.roms.is_empty() {
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

    fn move_by(&mut self, delta: isize) {
        let (index, len) = match self.focus {
            Focus::Roms => (&mut self.rom_index, self.roms.len()),
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

        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        Self::draw_header(frame, header);

        let [list_area, side] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)]).areas(body);
        self.draw_roms(frame, list_area);
        let [detail, folders] =
            Layout::vertical([Constraint::Length(10), Constraint::Fill(1)]).areas(side);
        self.draw_detail(frame, detail);
        self.draw_folders(frame, folders);
        self.draw_footer(frame, footer);
    }

    fn draw_header(frame: &mut Frame, area: Rect) {
        let logo: Vec<Line> = LOGO
            .iter()
            .map(|row| Line::styled(*row, theme::accent()))
            .collect();
        frame.render_widget(
            Paragraph::new(logo).block(Block::default().padding(Padding::new(2, 0, 0, 0))),
            area,
        );
        let tagline = Line::from(vec![
            Span::styled("GAME BOY ADVANCE", theme::text()),
            Span::styled("  ·  in your terminal  ", theme::dim()),
        ]);
        frame.render_widget(
            Paragraph::new(tagline)
                .alignment(Alignment::Right)
                .block(Block::default().padding(Padding::new(0, 0, 1, 0))),
            area,
        );
    }

    fn pane(title: &str, focused: bool) -> Block<'static> {
        let style = if focused {
            theme::accent()
        } else {
            theme::border()
        };
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border())
            .title(Line::styled(format!(" {title} "), style))
            .padding(Padding::horizontal(1))
    }

    fn draw_roms(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Roms;
        let block = Self::pane("LIBRARY", focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.roms.is_empty() {
            let text = if self.library.folders.is_empty() {
                "No folders yet.\n\nPress a to add the folder your .gba files live in."
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
        let items: Vec<ListItem> = self
            .roms
            .iter()
            .enumerate()
            .map(|(i, rom)| {
                let selected = i == self.rom_index;
                let name = truncate(&rom.name(), name_width);
                let code = rom
                    .header
                    .as_ref()
                    .map_or_else(|| "----".to_string(), |h| pad4(&h.game_code));
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
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{marker}{name:<name_width$}"), name_style),
                    Span::styled(format!("  {code}  "), meta_style),
                    Span::styled(save.to_string(), save_style),
                    Span::styled(" ", name_style),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.rom_index));
        frame.render_stateful_widget(List::new(items), inner, &mut state);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect) {
        let block = Self::pane("CARTRIDGE", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(rom) = self.roms.get_mut(self.rom_index) else {
            return;
        };
        let save_type = rom.save_type();
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

        let mut lines = vec![row("File", plain(rom.file_name()))];
        match &rom.header {
            Some(h) => {
                lines.push(row("Title", plain(h.title.clone())));
                lines.push(row(
                    "Code",
                    plain(format!("{}  ·  maker {}", pad4(&h.game_code), h.maker_code)),
                ));
                lines.push(row("Version", plain(format!("1.{}", h.version))));
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
        lines.push(row("Size", plain(human_size(rom.size))));
        let chip = match save_type {
            Some(SaveType::Sram) => "SRAM 32K",
            Some(SaveType::Flash64K) => "Flash 64K",
            Some(SaveType::Flash128K) => "Flash 128K",
            Some(SaveType::Eeprom) => "EEPROM",
            Some(SaveType::Unknown) => "none detected",
            None => "unreadable",
        };
        lines.push(row_spans(
            "Save",
            vec![
                plain(chip.to_string()),
                if rom.has_save {
                    Span::styled("  ·  has save ●", theme::text().fg(theme::OK))
                } else {
                    Span::styled("  ·  no save yet", theme::dim())
                },
            ],
        ));
        let path_width = usize::from(inner.width).saturating_sub(8);
        lines.push(row(
            "Path",
            Span::styled(
                truncate_start(&compact_home(&rom.path), path_width),
                theme::dim(),
            ),
        ));
        frame.render_widget(Paragraph::new(lines), inner);
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
            Mode::AddFolder(input) => Line::from(vec![
                Span::styled(" Add folder ", theme::key()),
                Span::styled(
                    format!(" {input}"),
                    Style::default().fg(theme::TEXT).bg(theme::SURFACE),
                ),
                Span::styled("█", Style::default().fg(theme::ACCENT).bg(theme::SURFACE)),
                Span::styled("   ⏎ confirm  esc cancel", theme::hint()),
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
        let mut picker = Picker::new(Library::default());
        picker.library.folders = folders;
        picker.roms = roms;
        picker.focus = Focus::Roms;
        picker
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
        assert_eq!(p.handle(key(KeyCode::Char('q'))), Some(Outcome::Quit));
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
        assert!(screen.contains("Title   TETRIS"), "{screen}");
        assert!(screen.contains("has save ●"), "{screen}");
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
