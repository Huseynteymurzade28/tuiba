//! Prints the key events crossterm reports, with the same terminal setup
//! tuiba uses. Press keys for six seconds; the log lands on the shell
//! afterwards, and in `tuiba-keylog.txt` in the temporary directory so
//! it can be attached to a bug report.
//!
//! Needs a real terminal: run it in a terminal window of its own.

use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

fn main() -> std::io::Result<()> {
    enable_raw_mode()?;
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
    }
    let start = Instant::now();
    let mut log = vec![format!("supports_keyboard_enhancement = {enhanced}")];
    while start.elapsed() < Duration::from_secs(6) {
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            log.push(format!(
                "{:>6} ms  {:?}  {:?}  {:?}",
                start.elapsed().as_millis(),
                key.code,
                key.kind,
                key.modifiers
            ));
        }
    }
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    let text = log.join("\n");
    println!("{text}");
    let path = std::env::temp_dir().join("tuiba-keylog.txt");
    std::fs::write(&path, text + "\n")?;
    println!("\nwritten to {}", path.display());
    Ok(())
}
