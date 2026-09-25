//! Performance figures for the status bar (`--stats`, `F3`).
//!
//! Meant for reports like "it stutters on my machine": they tell a slow
//! terminal (long draws, fewer frames shown than emulated) apart from a
//! slow emulator (fewer frames emulated) and from sound trouble (a
//! shallow queue, dry spells).

use std::time::{Duration, Instant};

/// Draw times gathered over one-second windows.
#[derive(Debug)]
pub struct Stats {
    window_start: Instant,
    draws: u32,
    total: Duration,
    worst: Duration,
    /// The last closed window.
    last: Window,
}

/// What one window measured.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Window {
    /// Frames drawn per second.
    rate: f64,
    average: Duration,
    worst: Duration,
}

/// The sound side, as [`crate::audio::AudioOutput::stats`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStats {
    /// Sound queued for the device, in milliseconds.
    pub queued_ms: u32,
    /// Times the queue has run dry since the device was opened.
    pub dry_spells: u32,
}

impl Stats {
    /// Starts measuring at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            draws: 0,
            total: Duration::ZERO,
            worst: Duration::ZERO,
            last: Window::default(),
        }
    }

    /// Records one frame drawn, which took `took` to hand to the
    /// terminal, and closes the window once a second has passed.
    pub fn draw(&mut self, took: Duration, now: Instant) {
        self.draws += 1;
        self.total += took;
        self.worst = self.worst.max(took);
        let elapsed = now.duration_since(self.window_start);
        if elapsed >= Duration::from_secs(1) {
            self.last = Window {
                rate: f64::from(self.draws) / elapsed.as_secs_f64(),
                average: self.total / self.draws,
                worst: self.worst,
            };
            *self = Self {
                last: self.last,
                ..Self::new(now)
            };
        }
    }

    /// The status-bar text, given the emulation rate and the sound side
    /// (`None` without a sound device).
    pub fn line(&self, emulated: f64, audio: Option<AudioStats>) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let sound = audio.map_or_else(
            || "no sound".to_string(),
            |a| format!("sound {} ms queued, {} dry", a.queued_ms, a.dry_spells),
        );
        format!(
            "emu {emulated:.1} fps · shown {:.1} fps · draw {:.1} ms avg {:.1} worst · {sound}",
            self.last.rate,
            ms(self.last.average),
            ms(self.last.worst),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_reports_rate_average_and_worst() {
        let start = Instant::now();
        let mut stats = Stats::new(start);
        let ms = Duration::from_millis;
        // Twenty draws over a second: 10 ms each but one of 50 ms.
        for i in 1..=20 {
            let took = if i == 7 { ms(50) } else { ms(10) };
            stats.draw(took, start + ms(50 * i));
        }
        assert_eq!(stats.last.worst, ms(50));
        assert_eq!(stats.last.average, ms(12));
        assert!((stats.last.rate - 20.0).abs() < 1e-9);
        assert_eq!(
            stats.line(
                59.73,
                Some(AudioStats {
                    queued_ms: 98,
                    dry_spells: 2
                })
            ),
            "emu 59.7 fps · shown 20.0 fps · draw 12.0 ms avg 50.0 worst · sound 98 ms queued, 2 dry"
        );
        // The next window starts from nothing.
        stats.draw(ms(5), start + ms(1100));
        assert_eq!(stats.worst, ms(5));
    }
}
