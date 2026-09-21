//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

mod cli;
mod crashlog;
mod graphics;
mod headless;
mod input;
mod library;
mod picker;
mod png;
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
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};
use tuiba_core::{Cartridge, Gba};

use crate::graphics::KittyGraphics;
use crate::input::{GbaKey, Keypad};
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
}

/// How often the save file is compared against backup memory and, if a
/// game has written to it, flushed to disk. A crash or a closed terminal
/// then costs at most this much progress.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(1);

impl App {
    /// Emulates one frame with the current keypad state.
    fn emulate_frame(&mut self, now: Instant) {
        self.gba.set_keyinput(self.keypad.keyinput(now));
        self.gba.run_frame();
        self.fps_frames += 1;
        let elapsed = now.duration_since(self.fps_window_start);
        if elapsed >= Duration::from_secs(1) {
            self.fps = f64::from(self.fps_frames) / elapsed.as_secs_f64();
            self.fps_frames = 0;
            self.fps_window_start = now;
        }
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
        let status = Line::from(vec![
            Span::styled(
                format!(" {}  ", self.title),
                theme::text().bg(theme::SURFACE),
            ),
            Span::styled(
                format!(
                    "{:.0} fps  {renderer}{size_hint}{keys}{swi_hint}{held}{save}",
                    self.fps
                ),
                Style::default().fg(theme::DIM).bg(theme::SURFACE),
            ),
            Span::styled("   esc ", theme::key()),
            Span::styled("library  ", theme::hint()),
            Span::styled(" ctrl+q ", theme::key()),
            Span::styled("quit", theme::hint()),
        ]);
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(theme::DIM).bg(theme::SURFACE)),
            status_area,
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

    let mut guard = TerminalGuard::enter()?;
    let TerminalGuard {
        terminal,
        release_events,
    } = &mut guard;
    match direct_rom {
        Some(rom) => play(terminal, &rom, *release_events, args.graphics).map(|_| ()),
        None => library_loop(terminal, library, *release_events, args.graphics),
    }
}

/// Library screen ⇄ game, until the user quits.
fn library_loop(
    terminal: &mut ratatui::DefaultTerminal,
    library: Library,
    release_events: bool,
    graphics: bool,
) -> Result<(), AppError> {
    let mut picker = Picker::new(library);
    loop {
        terminal.draw(|frame| picker.draw(frame))?;
        let outcome = match event::read()? {
            Event::Key(key) => picker.handle(key),
            _ => None,
        };
        match outcome {
            None => {}
            Some(Outcome::Quit) => return Ok(()),
            Some(Outcome::Play(rom)) => {
                match play(terminal, &rom, release_events, graphics) {
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
    release_events: bool,
    graphics: bool,
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
    let mut app = App {
        title,
        gba,
        keypad: Keypad::new(release_events),
        graphics: (graphics && graphics::terminal_supports_kitty_graphics())
            .then(KittyGraphics::new),
        screen_area: Rect::default(),
        fps_frames: 0,
        fps_window_start: Instant::now(),
        fps: 0.0,
        save_path: rom.with_extension("sav"),
        saved,
        save_error: None,
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
/// game never sees dropped input or jumps in time.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<GameExit, AppError> {
    let mut next_frame = Instant::now();
    let mut next_autosave = next_frame + AUTOSAVE_INTERVAL;
    loop {
        let now = Instant::now();
        app.emulate_frame(now);
        if now >= next_autosave {
            app.flush_save();
            next_autosave = now + AUTOSAVE_INTERVAL;
        }
        terminal.draw(|frame| app.draw(frame))?;
        if let Some(graphics) = &mut app.graphics {
            graphics.present(app.gba.framebuffer(), app.screen_area)?;
        }

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                // Esc leaves the game, Ctrl+Q the program; plain letters
                // belong to the game.
                if key.kind == KeyEventKind::Press {
                    if key.code == KeyCode::Esc {
                        return Ok(GameExit::Back);
                    }
                    if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(GameExit::Quit);
                    }
                }
                app.keypad.handle(key, Instant::now());
            }
        }

        next_frame += FRAME_PERIOD;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            // Running behind: resynchronise instead of trying to catch up.
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
