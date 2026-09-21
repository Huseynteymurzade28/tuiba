//! A trace of fatal errors, for when the terminal closes with the program.
//!
//! A panic or a fatal I/O error in a full-screen TUI is easy to miss: the
//! terminal may close, or the message scrolls away under the restored
//! screen. Everything fatal is therefore also appended to
//! `$XDG_STATE_HOME/tuiba/crash.log` (or `~/.local/state/tuiba/crash.log`),
//! and the panic hook remembers the message so the caller can show it
//! after the terminal has been restored.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The last panic message, kept for whoever catches the unwind.
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Where the log lives, when the environment says where state may go.
#[must_use]
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state"))
        })?;
    Some(base.join("tuiba").join("crash.log"))
}

/// Appends one entry to the log. Best effort: a failure to log must not
/// turn into another error to report.
pub fn record(kind: &str, message: &str) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(mut file) = fs::OpenOptions::new().append(true).create(true).open(&path) else {
        return;
    };
    let _ = writeln!(
        file,
        "[{}] tuiba {} {kind}: {message}\n",
        timestamp(),
        env!("CARGO_PKG_VERSION")
    );
}

/// Installs a panic hook that logs the panic with a backtrace and
/// remembers its message.
///
/// It replaces the default hook rather than chaining to it: printing to
/// stderr while the alternate screen is up only smears text over the UI,
/// and the message is lost anyway when the screen is restored. Whoever
/// catches the unwind reports the message once the terminal is back.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        let message = describe(info);
        let backtrace = std::backtrace::Backtrace::force_capture();
        record("panic", &format!("{message}\n{backtrace}"));
        if let Ok(mut last) = LAST_PANIC.lock() {
            *last = Some(message);
        }
    }));
}

/// Takes the message of the most recent panic, if one was recorded.
pub fn take_last_panic() -> Option<String> {
    LAST_PANIC.lock().ok().and_then(|mut last| last.take())
}

/// Turns a panic payload into a one-line message, or falls back to
/// what the hook recorded.
pub fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    text.or_else(take_last_panic)
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// The panic message with its location: `main.rs:12:3: index out of bounds`.
fn describe(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    match info.location() {
        Some(loc) => format!("{}:{}:{}: {message}", loc.file(), loc.line(), loc.column()),
        None => message,
    }
}

/// UTC time as `YYYY-MM-DD HH:MM:SS`, without pulling in a date crate.
fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (hour, min, sec) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

/// Days since 1970-01-01 → (year, month, day), Howard Hinnant's algorithm.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_717), (2026, 9, 21));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29), "leap day");
    }

    #[test]
    fn panic_payloads_become_text() {
        let boxed: Box<dyn std::any::Any + Send> = Box::new("plain");
        assert_eq!(panic_message(&boxed), "plain");
        let boxed: Box<dyn std::any::Any + Send> = Box::new(String::from("owned"));
        assert_eq!(panic_message(&boxed), "owned");
    }
}
