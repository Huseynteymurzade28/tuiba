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
use tuiba_core::{Cartridge, Framebuffer};

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

/// Frontend state.
struct App {
    title: String,
    framebuffer: Framebuffer,
    keypad: Keypad,
}

impl App {
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

        frame.render_widget(GbaScreen::new(&self.framebuffer), screen_area);

        let size_hint = if GbaScreen::fits(screen_area) {
            String::new()
        } else {
            format!(
                "  [terminal {}x{} < {}x{}: cropped]",
                screen_area.width,
                screen_area.height,
                screen::CELL_WIDTH,
                screen::CELL_HEIGHT
            )
        };
        let keys = if self.keypad.has_release_events() {
            "kitty"
        } else {
            "timeout"
        };
        let now = Instant::now();
        let status = Line::from(format!(
            " {}{size_hint}  keys:{keys} KEYINPUT={:#06x} [{}]  q: quit",
            self.title,
            self.keypad.keyinput(now),
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
        title: cartridge.header().title.clone(),
        framebuffer: Framebuffer::new(),
        keypad: Keypad::new(release_events),
    };
    let result = event_loop(&mut terminal, &mut app);

    if release_events {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<(), AppError> {
    loop {
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
        std::thread::sleep(Duration::from_millis(16));
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
