//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

mod audio;
mod bindings;
mod cli;
mod crashlog;
mod graphics;
mod headless;
mod input;
mod library;
mod picker;
mod png;
mod savestate;
mod screen;
mod theme;
mod wordmark;

use std::io::stdout;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use ratatui::{Frame, Terminal};
use tuiba_core::{Cartridge, Gba, Snapshot};

use crate::audio::AudioOutput;
use crate::bindings::{Action, Bindings};
use crate::graphics::KittyGraphics;
use crate::input::{GbaKey, Hold, Keypad};
use crate::library::Library;
use crate::picker::{Outcome, Picker};
use crate::screen::GbaScreen;

/// Errors specific to the terminal frontend.
#[derive(Debug, thiserror::Error)]
enum AppError {
    /// The command line could not be parsed.
    #[error(transparent)]
    Args(#[from] cli::ArgError),

    /// An error bubbled up from the emulator core.
    #[error(transparent)]
    Core(#[from] tuiba_core::GbaError),

    /// Terminal I/O failed.
    #[error("terminal error: {0}")]
    Io(#[from] std::io::Error),

    /// The game loop panicked; the message is what the panic said.
    #[error("crashed: {0}")]
    Crash(String),
}

/// A terminal in raw mode on the alternate screen, restored on drop so
/// that every exit path (including an unwinding panic) hands the shell
/// back a usable terminal.
struct TerminalGuard {
    terminal: ratatui::DefaultTerminal,
    release_events: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, AppError> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        // Ask for key release events; terminals that lack the protocol
        // simply ignore the request and we fall back to timeout-based
        // releases.
        let release_events = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if release_events {
            let _ = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
            );
        }
        Ok(Self {
            terminal,
            release_events,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.release_events {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        }
        // Raw mode first: it has more side effects than the alternate
        // screen, so it matters more that it gets undone.
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// Target frame period: the GBA runs at 16.78 MHz / 280 896 cycles per
/// frame ≈ 59.73 Hz.
const FRAME_PERIOD: Duration = Duration::from_micros(16_743);

/// Frontend state.
#[allow(clippy::struct_excessive_bools)] // independent toggles, not a state machine
struct App {
    title: String,
    gba: Gba,
    keypad: Keypad,
    /// Pixel output, when the terminal supports it; otherwise half-blocks.
    graphics: Option<KittyGraphics>,
    /// Where the last draw put the screen, for the graphics overlay.
    screen_area: Rect,
    /// Frames emulated in the current measurement window.
    fps_frames: u32,
    fps_window_start: Instant,
    /// Last measured emulation rate.
    fps: f64,
    /// The `.sav` next to the ROM.
    save_path: PathBuf,
    /// Backup memory as of the last save write (or load), so a change can
    /// be flushed without a dirty flag from the core.
    saved: Vec<u8>,
    /// Why the last save write failed, for the status bar.
    save_error: Option<String>,
    /// Emulation is stopped; `.` runs a single frame.
    paused: bool,
    /// A single frame was requested while paused.
    step: bool,
    /// Fast-forward key held: run uncapped.
    fast: Hold,
    /// Key → action table, from the defaults and the user's keys file.
    bindings: Bindings,
    /// The `?` overlay is up; emulation waits while it is.
    help: bool,
    /// When Esc was last pressed: a second press within
    /// [`LEAVE_WINDOW`] leaves the game, so a stray one cannot.
    leave_armed: Option<Instant>,
    /// The sound device, or why there is none.
    audio: Result<AudioOutput, String>,
    /// Sound switched off by the user; samples are dropped.
    muted: bool,
    /// The quick save state, kept in memory so reloading it costs
    /// nothing. The copy on disk is what survives leaving the game.
    state: Option<Snapshot>,
    /// Where this cartridge's quick slot is written, when there is a
    /// state directory to write it to.
    state_path: Option<PathBuf>,
    /// What the last save/load did and when, for the status bar.
    state_notice: Option<(String, Instant)>,
}

/// How long the first Esc keeps "press again to leave" open.
const LEAVE_WINDOW: Duration = Duration::from_secs(2);

/// How long "state saved" and friends stay in the status bar.
const NOTICE_WINDOW: Duration = Duration::from_secs(2);

/// Nominal GBA frame rate, for the fast-forward multiplier.
const NOMINAL_FPS: f64 = 1_000_000.0 / 16_743.0;

/// How often the save file is compared against backup memory and, if a
/// game has written to it, flushed to disk. A crash or a closed terminal
/// then costs at most this much progress.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(1);

impl App {
    /// Emulates one frame with the current keypad state.
    fn emulate_frame(&mut self, now: Instant) {
        self.gba.set_keyinput(self.keypad.keyinput(now));
        self.gba.run_frame();
        // Fast-forward makes far more sound than real time can play;
        // dropping it whole is less jarring than playing chopped-up bits.
        if let Ok(audio) = &self.audio
            && !self.muted
            && !self.fast.is_held(now)
        {
            audio.push(self.gba.audio());
        }
        self.gba.clear_audio();
        self.fps_frames += 1;
        let elapsed = now.duration_since(self.fps_window_start);
        if elapsed >= Duration::from_secs(1) {
            self.fps = f64::from(self.fps_frames) / elapsed.as_secs_f64();
            self.fps_frames = 0;
            self.fps_window_start = now;
        }
    }

    /// Toggles pause. The rate counter restarts so it does not show a
    /// stale figure next to "paused", or a partial one after resuming.
    fn toggle_pause(&mut self, now: Instant) {
        self.paused = !self.paused;
        self.step = false;
        self.reset_fps(now);
    }

    /// Toggles the `?` overlay, which also stops emulation.
    fn toggle_help(&mut self, now: Instant) {
        self.help = !self.help;
        self.reset_fps(now);
    }

    /// Freezes the machine into the quick slot, replacing whatever was
    /// there, and writes it out so it survives the session.
    ///
    /// A state that cannot reach the disk is still in memory and still
    /// loadable now; the status bar says so rather than pretending the
    /// save was clean.
    fn save_state(&mut self, now: Instant) {
        let snapshot = self.gba.snapshot();
        let notice = match &self.state_path {
            Some(path) => match savestate::write(path, &snapshot) {
                Ok(()) => "state saved".to_string(),
                Err(err) => {
                    crashlog::record(
                        "state",
                        &format!("could not write {}: {err}", path.display()),
                    );
                    format!("state saved, but not to disk: {err}")
                }
            },
            None => "state saved (no state directory; this session only)".to_string(),
        };
        self.state = Some(snapshot);
        self.state_notice = Some((notice, now));
    }

    /// Puts the quick slot back: the one in memory, or failing that the
    /// one on disk from an earlier session.
    ///
    /// The sound queue is flushed with it: it holds samples from the
    /// moment being replaced, and playing them after the jump is a
    /// click. Backup memory comes back with the state, so the next
    /// autosave writes the `.sav` the state expects.
    fn load_state(&mut self, now: Instant) {
        if self.state.is_none() {
            match self.read_state_from_disk() {
                Ok(snapshot) => self.state = snapshot,
                Err(err) => {
                    self.state_notice = Some((err, now));
                    return;
                }
            }
        }
        let Some(snapshot) = &self.state else {
            self.state_notice = Some(("no state saved yet".to_string(), now));
            return;
        };
        self.gba.restore(snapshot);
        if let Ok(audio) = &self.audio {
            audio.flush();
        }
        self.reset_fps(now);
        self.state_notice = Some(("state loaded".to_string(), now));
    }

    /// The state file for this cartridge, if there is one to read.
    fn read_state_from_disk(&self) -> Result<Option<Snapshot>, String> {
        let Some(path) = &self.state_path else {
            return Ok(None);
        };
        savestate::read(path, &self.gba.bus.cartridge)
    }

    fn reset_fps(&mut self, now: Instant) {
        self.fps = 0.0;
        self.fps_frames = 0;
        self.fps_window_start = now;
    }

    /// Writes the save file if backup memory changed since the last
    /// write. Goes through a temporary file so a crash mid-write cannot
    /// leave a truncated save behind.
    fn flush_save(&mut self) {
        let data = self.gba.save_data();
        if data == self.saved.as_slice() {
            return;
        }
        let tmp = self.save_path.with_extension("sav.tmp");
        let written =
            std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, &self.save_path));
        match written {
            Ok(()) => {
                self.saved = data.to_vec();
                self.save_error = None;
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                self.save_error = Some(err.to_string());
            }
        }
    }

    /// Human-readable list of held buttons, for the status bar.
    fn held_buttons(&self, now: Instant) -> String {
        GbaKey::ALL
            .iter()
            .filter(|&&k| self.keypad.is_pressed(k, now))
            .map(|k| format!("{k:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [screen_area, status_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        self.screen_area = screen_area;

        frame.render_widget(Block::default().style(theme::text()), screen_area);
        let size_hint = if self.graphics.is_some() {
            // The image is overlaid after the frame is flushed; the cells
            // underneath stay blank.
            String::new()
        } else {
            frame.render_widget(GbaScreen::new(self.gba.framebuffer()), screen_area);
            let scale = GbaScreen::scale_for(screen_area);
            if !GbaScreen::fits(screen_area) {
                format!(
                    "  [terminal {}x{} too small: cropped]",
                    screen_area.width, screen_area.height
                )
            } else if scale > 1 {
                format!(
                    "  [1/{scale} scale; {}x{} for full]",
                    screen::CELL_WIDTH,
                    screen::CELL_HEIGHT
                )
            } else {
                String::new()
            }
        };
        let renderer = if self.graphics.is_some() {
            "pixels"
        } else {
            "half-blocks"
        };
        let now = Instant::now();
        let swi_hint = self
            .gba
            .last_unsupported_swi
            .map(|n| format!("  [unsupported SWI {n:#04x}]"))
            .unwrap_or_default();
        let held = self.held_buttons(now);
        let held = if held.is_empty() {
            String::new()
        } else {
            format!("  [{held}]")
        };
        // Without release events a key counts as held until it times out;
        // worth knowing when a game feels sticky.
        let keys = if self.keypad.has_release_events() {
            ""
        } else {
            "  keys: timeout"
        };
        let save = self
            .save_error
            .as_ref()
            .map(|err| format!("  [save failed: {err}]"))
            .unwrap_or_default();
        let sound = match &self.audio {
            _ if self.muted => "  🔇 muted",
            Ok(_) => "",
            Err(_) => "  🔇 no audio",
        };
        let notice = self
            .state_notice
            .as_ref()
            .filter(|(_, at)| now < *at + NOTICE_WINDOW)
            .map(|(text, _)| format!("  [{text}]"))
            .unwrap_or_default();
        let mode = if self.help {
            "  ⏸ keys".to_string()
        } else if self.leave_armed.is_some_and(|t| now < t + LEAVE_WINDOW) {
            "  esc again to leave".to_string()
        } else if self.paused {
            "  ⏸ paused  (. = one frame)".to_string()
        } else if self.fast.is_held(now) {
            format!("  ▶▶ ×{:.1}", self.fps / NOMINAL_FPS)
        } else {
            String::new()
        };
        let status = Line::from(vec![
            Span::styled(
                format!(" {}  ", self.title),
                theme::text().bg(theme::SURFACE),
            ),
            Span::styled(
                format!(
                    "{:.0} fps{mode}  {renderer}{size_hint}{keys}{sound}{notice}{swi_hint}{held}{save}",
                    self.fps
                ),
                Style::default().fg(theme::DIM).bg(theme::SURFACE),
            ),
            Span::styled("   ? ", theme::key()),
            Span::styled("keys  ", theme::hint()),
            Span::styled(" esc esc ", theme::key()),
            Span::styled("library  ", theme::hint()),
            Span::styled(" ctrl+q ", theme::key()),
            Span::styled("quit", theme::hint()),
        ]);
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(theme::DIM).bg(theme::SURFACE)),
            status_area,
        );
        if self.help {
            self.draw_help(frame, screen_area);
        }
    }

    /// The `?` overlay: every action with its keys, and where to change
    /// them.
    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Action::ALL
            .iter()
            .map(|&action| {
                let keys = self.bindings.keys_for(action);
                let keys = if keys.is_empty() {
                    Span::styled("(unbound)", theme::dim())
                } else {
                    Span::styled(keys.join("  "), theme::text())
                };
                Line::from(vec![
                    Span::styled(format!("{:>20}  ", action.label()), theme::dim()),
                    keys,
                ])
            })
            .collect();
        lines.push(Line::default());
        for (label, key) in [
            ("library", "esc esc"),
            ("quit", "ctrl+q"),
            ("this list", "?"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:>20}  "), theme::dim()),
                Span::styled(key, theme::text()),
            ]));
        }
        lines.push(Line::default());
        let file = Bindings::file().map_or_else(
            || "keys file needs a config directory".to_string(),
            |p| format!("edit {}", library::compact_home(&p)),
        );
        lines.push(Line::styled(file, theme::hint()).alignment(Alignment::Center));
        // Wide enough for the longest line (usually the path), within
        // the screen.
        let width = lines.iter().map(Line::width).max().unwrap_or(0).max(44) as u16 + 4;
        let width = width.min(area.width);

        let height = lines.len() as u16 + 2;
        let [popup] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .title(Line::styled(" KEYS ", theme::accent().bold()))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::border())
                    .padding(Padding::horizontal(1))
                    .style(theme::text().bg(theme::SURFACE)),
            ),
            popup,
        );
    }
}

/// How the game loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameExit {
    /// Esc: back to wherever we came from.
    Back,
    /// Ctrl+Q: leave the program.
    Quit,
}

fn run() -> Result<(), AppError> {
    let args = cli::Args::parse(std::env::args().skip(1))?;

    // Headless runs read the save (so the game boots past its checks) but
    // never write it: a debugging session must not clobber real progress.
    if let Some(config) = &args.headless {
        let rom = args
            .rom
            .as_deref()
            .expect("parser requires a ROM for headless flags");
        let mut gba = load_gba(rom)?;
        return Ok(headless::run(&mut gba, config)?);
    }

    // A ROM plays directly; a folder (or nothing) opens the library.
    let mut library = Library::load();
    let direct_rom = match &args.rom {
        Some(path) if path.is_dir() => {
            if library.add(path.clone()) {
                library.save()?;
            }
            None
        }
        Some(path) => Some(path.clone()),
        None => None,
    };

    let (bindings, key_problems) = Bindings::load();
    if direct_rom.is_some() {
        // No library footer to show these in; they stay in the scrollback.
        for problem in &key_problems {
            eprintln!("warning: {problem}");
        }
    }

    let mut guard = TerminalGuard::enter()?;
    let TerminalGuard {
        terminal,
        release_events,
    } = &mut guard;
    let session = Session {
        release_events: *release_events,
        graphics: args.graphics,
        sound: !args.mute,
        bindings,
    };
    if let Some(rom) = direct_rom {
        return play(terminal, &rom, &session).map(|_| ());
    }
    let mut picker = Picker::new(library);
    if let Some(problem) = key_problems.first() {
        picker.notify_error(problem.clone());
    }
    library_loop(terminal, picker, &session)
}

/// Settings that hold for every game played in this run.
struct Session {
    /// The terminal reports key releases.
    release_events: bool,
    /// Use the terminal's graphics protocol when available.
    graphics: bool,
    /// Open the sound device.
    sound: bool,
    bindings: Bindings,
}

/// After a game hands control back, Esc and q are ignored until this
/// long has passed without either being seen. The Esc that left the game
/// is usually still held, and terminals without release events report
/// its repeats as fresh presses; the first one arrives after the OS
/// repeat delay, up to ~660 ms on X11.
const QUIT_KEY_COOLDOWN: Duration = Duration::from_millis(750);

/// Library screen ⇄ game, until the user quits.
fn library_loop(
    terminal: &mut ratatui::DefaultTerminal,
    mut picker: Picker,
    session: &Session,
) -> Result<(), AppError> {
    let mut quit_keys_muted_until = Instant::now();
    loop {
        terminal.draw(|frame| picker.draw(frame))?;
        let outcome = match event::read()? {
            Event::Key(key)
                if !session.release_events
                    && matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) =>
            {
                let now = Instant::now();
                if now < quit_keys_muted_until {
                    quit_keys_muted_until = now + QUIT_KEY_COOLDOWN;
                    continue;
                }
                picker.handle(key)
            }
            Event::Key(key) => picker.handle(key),
            _ => None,
        };
        match outcome {
            None => {}
            Some(Outcome::Quit) => return Ok(()),
            Some(Outcome::Play(rom)) => {
                picker.mark_played(&rom);
                match play(terminal, &rom, session) {
                    Ok(GameExit::Back) => {}
                    Ok(GameExit::Quit) => return Ok(()),
                    // A broken ROM, or a bug it trips over, should not
                    // take the library down with it.
                    Err(AppError::Core(err)) => picker.notify_error(err.to_string()),
                    Err(err @ AppError::Crash(_)) => {
                        picker.notify_error(format!("{err} (see {})", crash_log_hint()));
                    }
                    Err(err) => return Err(err),
                }
                quit_keys_muted_until = Instant::now() + QUIT_KEY_COOLDOWN;
                // No explicit clear: the next draw diffs against the game's
                // last frame and repaints every cell that differs. (Ratatui's
                // `clear` also queries the cursor position, which some
                // terminals never answer, killing the loop with an I/O error.)
                picker.rescan();
            }
        }
    }
}

/// Where to point the user for details of a crash.
fn crash_log_hint() -> String {
    crashlog::path().map_or_else(
        || "crash log unavailable".to_string(),
        |p| library::compact_home(&p),
    )
}

/// Loads a cartridge and its save file.
fn load_gba(rom: &Path) -> Result<Gba, AppError> {
    let cartridge = Cartridge::load(rom)?;
    let mut gba = Gba::new(cartridge);
    if let Ok(data) = std::fs::read(rom.with_extension("sav")) {
        gba.load_save_data(&data);
    }
    Ok(gba)
}

/// Runs one cartridge until the user leaves it, then writes its save.
///
/// A panic inside the game loop is caught here: the save is flushed, the
/// terminal stays in raw mode for the library, and the message is
/// reported as [`AppError::Crash`] (the panic hook has already logged it).
fn play(
    terminal: &mut ratatui::DefaultTerminal,
    rom: &Path,
    session: &Session,
) -> Result<GameExit, AppError> {
    let gba = load_gba(rom)?;
    let header_title = gba.bus.cartridge.header().title.trim().to_string();
    let title = if header_title.is_empty() {
        // Untitled homebrew: fall back to the file name.
        rom.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        header_title
    };
    // Backup memory as loaded: a `.sav` is only ever written once the game
    // changes it, so cartridges that never save do not grow one.
    let saved = gba.save_data().to_vec();
    let state_path = savestate::slot_path(rom, &gba.bus.cartridge);
    let mut app = App {
        title,
        gba,
        keypad: Keypad::new(session.release_events),
        graphics: (session.graphics && graphics::terminal_supports_kitty_graphics())
            .then(KittyGraphics::new),
        screen_area: Rect::default(),
        fps_frames: 0,
        fps_window_start: Instant::now(),
        fps: 0.0,
        save_path: rom.with_extension("sav"),
        saved,
        save_error: None,
        paused: false,
        step: false,
        fast: Hold::new(session.release_events),
        bindings: session.bindings.clone(),
        help: false,
        leave_armed: None,
        audio: if session.sound {
            AudioOutput::open().map_err(|e| e.to_string())
        } else {
            Err("muted".into())
        },
        muted: !session.sound,
        state: None,
        state_path,
        state_notice: None,
    };
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| event_loop(terminal, &mut app)));

    // Persist the save however the loop ended.
    app.flush_save();
    if let Some(err) = &app.save_error {
        crashlog::record(
            "save",
            &format!("could not write {}: {err}", app.save_path.display()),
        );
    }
    match result {
        Ok(result) => result,
        Err(payload) => Err(AppError::Crash(crashlog::panic_message(&payload))),
    }
}

/// Emulate → draw → handle input, paced to the GBA's frame rate.
///
/// Emulation and rendering are decoupled: if a frame takes longer than
/// the period we simply run late rather than skipping emulation, so the
/// game never sees dropped input or jumps in time. Fast-forward keeps
/// drawing at the usual rate but fills each period with as many frames
/// as the machine manages; pause keeps the loop (and input) alive with
/// no emulation at all.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<GameExit, AppError> {
    let mut next_frame = Instant::now();
    let mut next_autosave = next_frame + AUTOSAVE_INTERVAL;
    loop {
        let now = Instant::now();
        if app.help {
            // The overlay covers the screen; nothing to see, so nothing to run.
        } else if app.paused {
            if app.step {
                app.emulate_frame(now);
                app.step = false;
            }
        } else if app.fast.is_held(now) {
            let deadline = now + FRAME_PERIOD;
            loop {
                let now = Instant::now();
                app.emulate_frame(now);
                if now >= deadline {
                    break;
                }
            }
        } else {
            app.emulate_frame(now);
        }
        if now >= next_autosave {
            app.flush_save();
            next_autosave = now + AUTOSAVE_INTERVAL;
        }
        terminal.draw(|frame| app.draw(frame))?;
        if let Some(graphics) = &mut app.graphics {
            if app.help {
                // The pixel image would sit on top of the overlay.
                graphics.clear()?;
            } else {
                graphics.present(app.gba.framebuffer(), app.screen_area)?;
            }
        }

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                let now = Instant::now();
                // Esc, Ctrl+Q and ? are fixed; everything else goes through
                // the bindings.
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc if app.help => {
                            app.toggle_help(now);
                            continue;
                        }
                        // Twice within the window: one Esc is too easy to
                        // hit by accident to throw a game away on.
                        KeyCode::Esc => {
                            if app.leave_armed.is_some_and(|t| now < t + LEAVE_WINDOW) {
                                return Ok(GameExit::Back);
                            }
                            app.leave_armed = Some(now);
                            continue;
                        }
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(GameExit::Quit);
                        }
                        KeyCode::Char('?') => {
                            app.toggle_help(now);
                            continue;
                        }
                        _ => {}
                    }
                }
                // Chords like Ctrl+Z stay with the terminal.
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    continue;
                }
                match app.bindings.action(key.code) {
                    Some(Action::Button(button)) => app.keypad.handle(button, key.kind, now),
                    Some(Action::FastForward) => app.fast.update(key.kind, now),
                    Some(Action::Mute) if key.kind == KeyEventKind::Press => {
                        app.muted = !app.muted;
                    }
                    Some(Action::Pause) if key.kind == KeyEventKind::Press => {
                        app.toggle_pause(now);
                    }
                    Some(Action::Step) if key.kind == KeyEventKind::Press && app.paused => {
                        app.step = true;
                    }
                    Some(Action::SaveState) if key.kind == KeyEventKind::Press => {
                        app.save_state(now);
                    }
                    Some(Action::LoadState) if key.kind == KeyEventKind::Press => {
                        app.load_state(now);
                    }
                    _ => {}
                }
            }
        }

        next_frame += FRAME_PERIOD;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            // Running behind (or fast-forwarding): resynchronise instead
            // of trying to catch up.
            next_frame = now;
        }
    }
}

fn main() -> ExitCode {
    crashlog::install_panic_hook();
    // A panic anywhere unwinds through the terminal guard first, so by the
    // time we report it the shell has its screen back.
    let result = std::panic::catch_unwind(run)
        .unwrap_or_else(|payload| Err(AppError::Crash(crashlog::panic_message(&payload))));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(AppError::Args(cli::ArgError::Help)) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Err(AppError::Args(err)) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
        Err(err) => {
            // The terminal guard has been dropped by now, so this lands on
            // the shell's screen rather than the alternate one.
            if !matches!(err, AppError::Crash(_)) {
                crashlog::record("error", &err.to_string());
            }
            eprintln!("error: {err}");
            eprintln!("details in {}", crash_log_hint());
            ExitCode::FAILURE
        }
    }
}
