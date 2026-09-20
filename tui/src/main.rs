//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

mod screen;

use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use tuiba_core::{Cartridge, Framebuffer};

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
}

impl App {
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
        let status = Line::from(format!(" {}{size_hint}  q: quit", self.title));
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::Black).bg(Color::Gray)),
            status_area,
        );
    }
}

fn run() -> Result<(), AppError> {
    let rom_path = std::env::args().nth(1).ok_or(AppError::Usage)?;
    let cartridge = Cartridge::load(&rom_path)?;
    let app = App {
        title: cartridge.header().title.clone(),
        framebuffer: Framebuffer::new(),
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &App) -> Result<(), AppError> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        {
            return Ok(());
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
