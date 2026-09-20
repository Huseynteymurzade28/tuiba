//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

use std::process::ExitCode;

/// Errors specific to the terminal frontend.
#[derive(Debug, thiserror::Error)]
enum AppError {
    /// No ROM path was supplied on the command line.
    #[error("usage: tuiba <rom.gba>")]
    Usage,

    /// An error bubbled up from the emulator core.
    #[error(transparent)]
    Core(#[from] tuiba_core::GbaError),
}

fn run() -> Result<(), AppError> {
    let rom_path = std::env::args().nth(1).ok_or(AppError::Usage)?;
    println!(
        "tuiba v{} – would load {rom_path}",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
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
