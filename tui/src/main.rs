//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

mod input;
mod screen;

use std::io::stdout;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use tuiba_core::{Cartridge, Gba};

use crate::input::{GbaKey, Keypad};
use crate::screen::GbaScreen;

/// Errors specific to the terminal frontend.
#[derive(Debug, thiserror::Error)]
enum AppError {
    /// No ROM path was supplied on the command line.
    #[error("usage: tuiba <rom.gba>")]
    Usage,

    /// An error bubbled up from the emulator core.
    #[error(transparent)]
    Core(#[from] tuiba_core::GbaError),

    /// Terminal I/O failed.
    #[error("terminal error: {0}")]
    Io(#[from] std::io::Error),
}

/// Target frame period: the GBA runs at 16.78 MHz / 280 896 cycles per
/// frame ≈ 59.73 Hz.
const FRAME_PERIOD: Duration = Duration::from_micros(16_743);

/// Frontend state.
struct App {
    title: String,
    gba: Gba,
    keypad: Keypad,
    /// Frames emulated in the current measurement window.
    fps_frames: u32,
    fps_window_start: Instant,
    /// Last measured emulation rate.
    fps: f64,
}

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

    /// Human-readable list of held buttons, for the status bar.
    fn held_buttons(&self, now: Instant) -> String {
        GbaKey::ALL
            .iter()
            .filter(|&&k| self.keypad.is_pressed(k, now))
            .map(|k| format!("{k:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn draw(&self, frame: &mut Frame) {
        let [screen_area, status_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

        frame.render_widget(GbaScreen::new(self.gba.framebuffer()), screen_area);

        let scale = GbaScreen::scale_for(screen_area);
        let size_hint = if !GbaScreen::fits(screen_area) {
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
        };
        let keys = if self.keypad.has_release_events() {
            "kitty"
        } else {
            "timeout"
        };
        let now = Instant::now();
        let swi_hint = self
            .gba
            .last_unsupported_swi
            .map(|n| format!("  [unsupported SWI {n:#04x}]"))
            .unwrap_or_default();
        let debug = format!(
            "pc={:#010x} {:?}{} dispcnt={:#06x}",
            self.gba.cpu.next_pc(),
            self.gba.cpu.regs.mode(),
            if self.gba.cpu.halted { " halt" } else { "" },
            self.gba.bus.io.read16(tuiba_core::memory::io::reg::DISPCNT),
        );
        let status = Line::from(format!(
            " {}{size_hint}{swi_hint}  {:.1} fps  {debug}  keys:{keys} [{}]  q: quit",
            self.title,
            self.fps,
            self.held_buttons(now)
        ));
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::Black).bg(Color::Gray)),
            status_area,
        );
    }
}

fn run() -> Result<(), AppError> {
    let rom_path = std::env::args().nth(1).ok_or(AppError::Usage)?;
    let cartridge = Cartridge::load(&rom_path)?;
    let save_path = std::path::Path::new(&rom_path).with_extension("sav");
    let mut gba = Gba::new(cartridge);
    if let Ok(data) = std::fs::read(&save_path) {
        gba.load_save_data(&data);
    }

    let mut terminal = ratatui::init();
    // Ask for key release events; terminals that lack the protocol simply
    // ignore the request and we fall back to timeout-based releases.
    let release_events = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if release_events {
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        );
    }

    let mut app = App {
        title: gba.bus.cartridge.header().title.clone(),
        gba,
        keypad: Keypad::new(release_events),
        fps_frames: 0,
        fps_window_start: Instant::now(),
        fps: 0.0,
    };
    let result = event_loop(&mut terminal, &mut app);

    if release_events {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();

    // Persist the save even if the loop ended with an error.
    if let Err(err) = std::fs::write(&save_path, app.gba.save_data()) {
        eprintln!("warning: could not write {}: {err}", save_path.display());
    }
    result
}

/// Emulate → draw → handle input, paced to the GBA's frame rate.
///
/// Emulation and rendering are decoupled: if a frame takes longer than
/// the period we simply run late rather than skipping emulation, so the
/// game never sees dropped input or jumps in time.
fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<(), AppError> {
    let mut next_frame = Instant::now();
    loop {
        let now = Instant::now();
        app.emulate_frame(now);
        terminal.draw(|frame| app.draw(frame))?;

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    return Ok(());
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
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
