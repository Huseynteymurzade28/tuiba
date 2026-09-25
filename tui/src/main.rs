//! Terminal frontend for the `tuiba` Game Boy Advance emulator.

mod audio;
mod bindings;
mod cli;
mod clock;
mod crashlog;
mod gamepad;
mod graphics;
mod headless;
mod input;
mod library;
mod picker;
mod png;
mod rewind;
mod savestate;
mod screen;
mod screenshot;
mod states;
mod stats;
mod theme;
mod wordmark;

use std::cell::Cell;
use std::io::stdout;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
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
use tuiba_core::patch::Format;
use tuiba_core::{Cartridge, Gba, Snapshot};

use crate::audio::AudioOutput;
use crate::bindings::{Action, Bindings, Input};
use crate::cli::FastSpeed;
use crate::gamepad::{Gamepads, PadButton, PadNotice, PadSet, PadStyle};
use crate::graphics::{Graphics, Renderer};
use crate::input::{GbaKey, Hold, Keypad};
use crate::library::Library;
use crate::picker::{Outcome, Picker};
use crate::rewind::Rewind;
use crate::screen::GbaScreen;
use crate::states::StatesPanel;
use crate::stats::Stats;

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

    /// The patch next to the ROM could not be applied.
    #[error("{name}: {err}")]
    Patch {
        /// The patch's file name.
        name: String,
        /// Why it failed.
        err: tuiba_core::GbaError,
    },

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
        // Ask for key release events, and for Esc to be sent as its own
        // escape sequence rather than a bare `\x1b`: without that, a
        // terminal has no way to tell us a press from a release and one
        // press of Esc arrives as two indistinguishable events.
        // Terminals that lack the protocol ignore the request and we
        // fall back to timeout-based releases.
        let release_events = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if release_events {
            let _ = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                )
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
    graphics: Option<Graphics>,
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
    /// Rewind key held: play backwards.
    rewind_key: Hold,
    /// The recent past, for rewinding.
    rewind: Rewind,
    /// Rewinding as of the last frame, so the first step of a rewind
    /// can flush the sound queue.
    rewinding: bool,
    /// Pad buttons held as of the last poll; a press is a button that
    /// was not in here.
    pads: PadSet,
    /// Pad buttons that were already down when the game started — the A
    /// that chose it in the library — ignored until they are let go of.
    pads_ignored: PadSet,
    /// The connected pad's labels, `None` without a pad; hints name its
    /// buttons while there is one.
    pad_style: Option<PadStyle>,
    /// When a bound `leave` (the pad's guide button) was last pressed:
    /// like Esc, it takes a second press to leave.
    bound_leave: Option<Instant>,
    /// Key → action table, from the defaults and the user's keys file.
    bindings: Bindings,
    /// The `?` overlay is up; emulation waits while it is.
    help: bool,
    /// When Esc was last pressed: a second press within
    /// [`LEAVE_WINDOW`] leaves the game, so a stray one cannot.
    leave_armed: Option<Instant>,
    /// Whether that Esc has been let go of since. One press must not
    /// leave the game however many events the terminal makes of it —
    /// some send a key's press and its release as the same bare `\x1b`,
    /// and a held key repeats.
    esc_released: bool,
    /// Whether this terminal has actually delivered a key release. The
    /// enhancement query is answered by terminals that then never send
    /// one, and waiting forever for a release that is not coming would
    /// leave no way out of a game.
    seen_release: bool,
    /// The sound device, or why there is none.
    audio: Result<AudioOutput, String>,
    /// Sound switched off by the user; samples are dropped.
    muted: bool,
    /// Sound volume in percent; carried from game to game by the session.
    volume: u8,
    /// Performance figures, while they are shown in the status bar.
    stats: Option<Stats>,
    /// How fast fast-forward may run.
    fast_speed: FastSpeed,
    /// The cartridge being played, for naming its state files.
    rom: PathBuf,
    /// Which slot `F5` and `F8` act on, counting from zero. The panel
    /// moves it; it starts on the first slot.
    slot: usize,
    /// A state with nowhere to be written: when there is no state
    /// directory, states still work, they just do not outlive the game.
    held_state: Option<Snapshot>,
    /// The save-state panel, while it is up. Emulation waits for it.
    panel: Option<StatesPanel>,
    /// What the last save/load did (or which pad came or went) and
    /// when, for the status bar.
    state_notice: Option<(String, Instant)>,
}

/// How long the first Esc keeps "press again to leave" open.
const LEAVE_WINDOW: Duration = Duration::from_secs(2);

/// Shortest gap between the two Esc presses, where the terminal does not
/// report releases and a release is therefore not something we can wait
/// for. Two events this close together came from one press of the key,
/// not from two: a Kitty asked only for event types sends the press and
/// the release of one Esc about 80 ms apart, both as a bare `\x1b`.
const MIN_LEAVE_GAP: Duration = Duration::from_millis(250);

/// How long "state saved" and friends stay in the status bar.
const NOTICE_WINDOW: Duration = Duration::from_secs(2);

/// How much one press of `-` or `+` changes the volume, in percent.
const VOLUME_STEP: u8 = 10;

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
        // The wall clock, not emulated time: a cartridge clock keeps the
        // real date through pauses, fast-forward and rewinds, as the
        // battery-backed chip in a real cartridge would.
        self.gba.set_clock(clock::now());
        self.gba.run_frame();
        // Fast-forward makes far more sound than real time can play;
        // dropping it whole is less jarring than playing chopped-up bits.
        if let Ok(audio) = &self.audio
            && !self.muted
            && !self.fast_held(now)
        {
            audio.push(self.gba.audio(), self.volume);
        }
        self.gba.clear_audio();
        self.fps_frames += 1;
        self.update_fps(now);
    }

    /// Closes the rate window once a second has passed.
    fn update_fps(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.fps_window_start);
        if elapsed >= Duration::from_secs(1) {
            self.fps = f64::from(self.fps_frames) / elapsed.as_secs_f64();
            self.fps_frames = 0;
            self.fps_window_start = now;
        }
    }

    /// Whether fast-forward is held, on the keyboard or a pad.
    fn fast_held(&self, now: Instant) -> bool {
        self.fast.is_held(now) || self.pad_holds(Action::FastForward)
    }

    /// Whether rewind is held, on the keyboard or a pad.
    fn rewind_held(&self, now: Instant) -> bool {
        self.rewind_key.is_held(now) || self.pad_holds(Action::Rewind)
    }

    /// Whether a pad button bound to `action` is down.
    fn pad_holds(&self, action: Action) -> bool {
        self.pads
            .iter()
            .any(|b| self.bindings.pad_action(b) == Some(action))
    }

    /// Goes back one capture. Sound stops while rewinding: the queue is
    /// flushed as it starts, and nothing is played until it ends.
    fn rewind_step(&mut self, now: Instant) {
        if !self.rewinding
            && let Ok(audio) = &self.audio
        {
            audio.flush();
        }
        if let Some(snapshot) = self.rewind.step_back(&self.gba.bus.cartridge) {
            self.gba.restore(&snapshot);
            self.fps_frames += 1;
        }
        self.update_fps(now);
    }

    /// Reads the pads: held buttons go to the keypad, fresh presses of
    /// anything else do what the same key would. While the save-state
    /// panel or the help overlay is up, presses go to that instead.
    fn poll_pads(&mut self, gamepads: &mut Gamepads, now: Instant) -> Option<GameExit> {
        for notice in gamepads.poll() {
            let text = match notice {
                PadNotice::Connected(name) => format!("{name} connected"),
                PadNotice::Disconnected(name) => format!("{name} disconnected"),
            };
            self.state_notice = Some((text, now));
        }
        self.pad_style = gamepads.style();
        let held = gamepads.held();
        self.pads_ignored = self.pads_ignored.and(held);
        let held = held.since(self.pads_ignored);
        let pressed = held.since(self.pads);
        self.pads = held;
        let mut exit = None;
        for button in pressed.iter() {
            let action = self.bindings.pad_action(button);
            // A press that works a menu is not for the game: when the
            // menu closes under a button still held, the game must not
            // find its A or B already down.
            if self.panel.is_some() || self.help {
                self.pads_ignored.insert(button);
            }
            if self.panel.is_some() {
                // The button that opened the panel closes it again.
                let key = if action == Some(Action::States) {
                    Some(KeyCode::Esc)
                } else {
                    StatesPanel::pad_key(button, self.pad_style.unwrap_or(PadStyle::Generic))
                };
                if let Some(key) = key {
                    self.handle_panel_key(KeyEvent::new(key, KeyModifiers::NONE), now);
                }
            } else if self.help
                && (button == PadButton::Start
                    || Some(button) == self.pad_style.map(PadStyle::back))
                && action != Some(Action::Help)
            {
                self.toggle_help(now);
            } else if let Some(action) = action {
                exit = self.trigger(action, now);
                if exit.is_some() {
                    break;
                }
            }
        }
        let buttons = self
            .pads
            .since(self.pads_ignored)
            .iter()
            .filter_map(|b| match self.bindings.pad_action(b) {
                Some(Action::Button(button)) => Some(button),
                _ => None,
            })
            .collect::<Vec<_>>();
        self.keypad.set_pad(buttons);
        exit
    }

    /// Does what a one-shot action does on press. Buttons and
    /// fast-forward are held rather than pressed, so they are not here.
    fn trigger(&mut self, action: Action, now: Instant) -> Option<GameExit> {
        match action {
            Action::Mute => self.muted = !self.muted,
            Action::VolumeDown => self.set_volume(self.volume.saturating_sub(VOLUME_STEP), now),
            Action::VolumeUp => self.set_volume(self.volume + VOLUME_STEP, now),
            Action::Pause => self.toggle_pause(now),
            Action::Step if self.paused => self.step = true,
            Action::SaveState => self.save_state(now),
            Action::LoadState => self.load_state(now),
            Action::States => self.open_panel(now),
            Action::Screenshot => self.screenshot(now),
            Action::FastSpeed => {
                self.fast_speed = self.fast_speed.next();
                self.state_notice = Some((format!("fast-forward {}", self.fast_speed), now));
            }
            Action::Stats => {
                self.stats = match self.stats {
                    Some(_) => None,
                    None => Some(Stats::new(now)),
                };
            }
            Action::Help => self.toggle_help(now),
            Action::Leave => {
                if self
                    .bound_leave
                    .is_some_and(|t| now.duration_since(t) < LEAVE_WINDOW)
                {
                    return Some(GameExit::Back);
                }
                self.bound_leave = Some(now);
            }
            Action::Button(_) | Action::FastForward | Action::Rewind | Action::Step => {}
        }
        None
    }

    /// Sets the volume (clamped to 100 %) and shows it. Turning it up
    /// also unmutes: the one wanting it louder wants to hear it.
    fn set_volume(&mut self, percent: u8, now: Instant) {
        self.volume = percent.min(100);
        self.muted = false;
        self.state_notice = Some((format!("volume {} %", self.volume), now));
    }

    /// How a hint should name what does `action`: the pad's button while
    /// one is connected and bound, otherwise `key`.
    fn hint_for(&self, action: Action, key: &'static str) -> &'static str {
        self.pad_style
            .zip(self.bindings.pad_for(action))
            .map_or(key, |(style, button)| style.label(button))
    }

    /// Saves the frame on screen and says where it went.
    fn screenshot(&mut self, now: Instant) {
        let saved = screenshot::dir()
            .ok_or_else(|| "no pictures directory".to_string())
            .and_then(|dir| {
                screenshot::save(&dir, &self.rom, self.gba.framebuffer())
                    .map_err(|err| err.to_string())
            });
        let notice = match saved {
            Ok(path) => format!("saved {}", library::compact_home(&path)),
            Err(err) => format!("screenshot failed: {err}"),
        };
        self.state_notice = Some((notice, now));
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

    /// Freezes the machine into the current slot and writes it out.
    ///
    /// A state that cannot reach the disk is still in memory and still
    /// loadable now; the status bar says so rather than pretending the
    /// save was clean.
    fn save_state(&mut self, now: Instant) {
        let snapshot = self.gba.snapshot();
        let notice = if let Some(path) = self.slot_path() {
            match savestate::write(&path, &snapshot) {
                Ok(()) => format!("slot {} saved", self.slot + 1),
                Err(err) => {
                    crashlog::record(
                        "state",
                        &format!("could not write {}: {err}", path.display()),
                    );
                    self.held_state = Some(snapshot.clone());
                    format!("slot {} saved, but not to disk: {err}", self.slot + 1)
                }
            }
        } else {
            self.held_state = Some(snapshot.clone());
            "state saved (no state directory; this session only)".to_string()
        };
        if let Some(panel) = &mut self.panel {
            panel.replace_selected(Some(snapshot));
        }
        self.state_notice = Some((notice, now));
    }

    /// Puts the current slot back into the machine.
    ///
    /// The sound queue is flushed with it: it holds samples from the
    /// moment being replaced, and playing them after the jump is a
    /// click. Backup memory comes back with the state, so the next
    /// autosave writes the `.sav` the state expects.
    fn load_state(&mut self, now: Instant) {
        match self.read_slot() {
            Ok(Some(snapshot)) => {
                let notice = format!("slot {} loaded", self.slot + 1);
                self.restore(&snapshot, now, notice);
            }
            Ok(None) => {
                self.state_notice = Some((format!("slot {} is empty", self.slot + 1), now));
            }
            Err(err) => self.state_notice = Some((err, now)),
        }
    }

    /// Puts a state back and tells the player it happened.
    fn restore(&mut self, snapshot: &Snapshot, now: Instant, notice: String) {
        self.gba.restore(snapshot);
        if let Ok(audio) = &self.audio {
            audio.flush();
        }
        self.reset_fps(now);
        self.state_notice = Some((notice, now));
    }

    /// Where the current slot lives, when there is a state directory.
    fn slot_path(&self) -> Option<PathBuf> {
        savestate::slot_path(&self.rom, &self.gba.bus.cartridge, self.slot + 1)
    }

    /// Reads the current slot: its file, or the state held in memory
    /// when there is nowhere to write one.
    fn read_slot(&self) -> Result<Option<Snapshot>, String> {
        let Some(path) = self.slot_path() else {
            return Ok(self.held_state.clone());
        };
        savestate::read(&path, &self.gba.bus.cartridge)
    }

    /// Opens the panel, which pauses the game the way the `?` overlay
    /// does.
    fn open_panel(&mut self, now: Instant) {
        self.panel = Some(StatesPanel::open(
            &self.rom,
            &self.gba.bus.cartridge,
            self.slot,
        ));
        self.reset_fps(now);
    }

    /// Handles one key while the panel is up, and acts on what it asks
    /// for.
    fn handle_panel_key(&mut self, key: crossterm::event::KeyEvent, now: Instant) {
        let Some(panel) = &mut self.panel else { return };
        // The cursor is the current slot, whether or not this key asked
        // for anything: F5 and F8 act on wherever the panel was left.
        let action = panel.handle(key);
        self.slot = panel.selected();
        let Some(action) = action else { return };
        match action {
            states::Action::Close => self.panel = None,
            states::Action::Save => self.save_state(now),
            states::Action::Load => {
                let snapshot = panel.selected_snapshot().cloned();
                match snapshot {
                    Some(snapshot) => {
                        let notice = format!("slot {} loaded", self.slot + 1);
                        self.restore(&snapshot, now, notice);
                        self.panel = None;
                    }
                    None => {
                        self.state_notice = Some((format!("slot {} is empty", self.slot + 1), now));
                    }
                }
            }
            states::Action::Delete => {
                let notice = match self.slot_path() {
                    Some(path) => match savestate::remove(&path) {
                        Ok(()) => format!("slot {} deleted", self.slot + 1),
                        Err(err) => format!("could not delete slot {}: {err}", self.slot + 1),
                    },
                    None => format!("slot {} deleted", self.slot + 1),
                };
                if let Some(panel) = &mut self.panel {
                    panel.replace_selected(None);
                }
                self.held_state = None;
                self.state_notice = Some((notice, now));
            }
        }
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

    /// The status bar: what the emulator is doing, and the three keys
    /// that are never rebindable.
    fn status_line(&self, size_hint: &str) -> Line<'_> {
        let renderer = self
            .graphics
            .as_ref()
            .map_or("half-blocks", |g| g.protocol().name());
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
        let pad = if self.pad_style.is_some() {
            "  🎮"
        } else {
            ""
        };
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
        } else if self.panel.is_some() {
            format!("  ⏸ states  (slot {})", self.slot + 1)
        } else if self.leave_armed.is_some_and(|t| now < t + LEAVE_WINDOW) {
            "  esc again to leave".to_string()
        } else if self.bound_leave.is_some_and(|t| now < t + LEAVE_WINDOW) {
            format!("  {} again to leave", self.hint_for(Action::Leave, "leave"))
        } else if self.paused {
            "  ⏸ paused  (. = one frame)".to_string()
        } else if self.rewinding && self.rewind.is_empty() {
            "  ◀◀ nothing older to rewind to".to_string()
        } else if self.rewinding {
            "  ◀◀ rewinding".to_string()
        } else if self.fast_held(now) {
            format!("  ▶▶ ×{:.1}", self.fps / NOMINAL_FPS)
        } else {
            String::new()
        };
        let mut spans = vec![
            Span::styled(
                format!(" {}  ", self.title),
                theme::text().bg(theme::SURFACE),
            ),
            Span::styled(
                format!(
                    "{:.0} fps{mode}  {renderer}{size_hint}{keys}{pad}{sound}{notice}{swi_hint}{held}{save}",
                    self.fps
                ),
                Style::default().fg(theme::DIM).bg(theme::SURFACE),
            ),
            Span::raw("  "),
        ];
        // The figures take the place of the key hints: they are for a
        // player who already knows the keys and is chasing a problem.
        if let Some(stats) = &self.stats {
            let audio = self.audio.as_ref().ok().map(AudioOutput::stats);
            spans.push(Span::styled(stats.line(self.fps, audio), theme::text()));
            return Line::from(spans);
        }
        spans.extend([
            Span::styled(
                format!(" {} ", self.hint_for(Action::Help, "?")),
                theme::key(),
            ),
            Span::styled("keys  ", theme::hint()),
            Span::styled(
                format!(" {} ", self.hint_for(Action::States, "f2")),
                theme::key(),
            ),
            Span::styled("states  ", theme::hint()),
            Span::styled(
                format!(" {} ×2 ", self.hint_for(Action::Leave, "esc")),
                theme::key(),
            ),
            Span::styled("library  ", theme::hint()),
            Span::styled(" ctrl+q ", theme::key()),
            Span::styled("quit", theme::hint()),
        ]);
        Line::from(spans)
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
        let status = self.status_line(&size_hint);
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(theme::DIM).bg(theme::SURFACE)),
            status_area,
        );
        if self.help {
            self.draw_help(frame, screen_area);
        }
        if let Some(panel) = &self.panel {
            panel.draw(frame, screen_area, self.pad_style);
        }
    }

    /// The `?` overlay: every action with its keys, and where to change
    /// them.
    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Action::ALL
            .iter()
            .map(|&action| {
                let mut spans = vec![Span::styled(
                    format!("{:>20}  ", action.label()),
                    theme::dim(),
                )];
                // A build that cannot read pads keeps quiet about them.
                let inputs = self
                    .bindings
                    .inputs_for(action)
                    .filter(|i| Gamepads::SUPPORTED || matches!(i, Input::Key(_)));
                if let Some(key) = action.fixed_key() {
                    spans.push(Span::styled(key, theme::text()));
                }
                for input in inputs {
                    if spans.len() > 1 {
                        spans.push(Span::raw("  "));
                    }
                    // A connected pad's buttons as printed on it; without
                    // one, the names the keys file takes.
                    spans.push(match (input, self.pad_style) {
                        (Input::Pad(button), Some(style)) => {
                            Span::styled(format!(" {} ", style.label(button)), theme::key())
                        }
                        (Input::Pad(_), None) => {
                            Span::styled(bindings::input_name(input), theme::accent())
                        }
                        (Input::Key(_), _) => {
                            Span::styled(bindings::input_name(input), theme::text())
                        }
                    });
                }
                if spans.len() == 1 {
                    spans.push(Span::styled("(unbound)", theme::dim()));
                }
                Line::from(spans)
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(format!("{:>20}  ", "quit"), theme::dim()),
            Span::styled("ctrl+q", theme::text()),
        ]));
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
        let (mut gba, _) = load_gba(rom)?;
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

    let mut gamepads = Gamepads::open();
    let mut guard = TerminalGuard::enter()?;
    let TerminalGuard {
        terminal,
        release_events,
    } = &mut guard;
    let session = Session {
        release_events: *release_events,
        renderer: args.renderer,
        sound: !args.mute,
        volume: Cell::new(args.volume),
        stats: Cell::new(args.stats),
        fast_speed: Cell::new(args.fast_speed),
        bindings,
    };
    if let Some(rom) = direct_rom {
        return play(terminal, &rom, &session, &mut gamepads).map(|_| ());
    }
    let mut picker = Picker::new(library);
    if let Some(problem) = key_problems.first() {
        picker.notify_error(problem.clone());
    }
    library_loop(terminal, picker, &session, &mut gamepads)
}

/// Settings that hold for every game played in this run.
struct Session {
    /// The terminal reports key releases.
    release_events: bool,
    /// How to draw the game screen.
    renderer: Renderer,
    /// Open the sound device.
    sound: bool,
    /// Sound volume in percent, as last set in a game.
    volume: Cell<u8>,
    /// Performance figures shown, as last set in a game.
    stats: Cell<bool>,
    /// Fast-forward limit, as last set in a game.
    fast_speed: Cell<FastSpeed>,
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
    gamepads: &mut Gamepads,
) -> Result<(), AppError> {
    let mut quit_keys_muted_until = Instant::now();
    let mut pads = gamepads.held();
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|frame| picker.draw(frame))?;
            dirty = false;
        }
        // Pads cannot wake `event::read`, so the terminal is waited on in
        // short slices with the pads read in between.
        let mut keys = Vec::new();
        if event::poll(LIBRARY_PAD_POLL)? {
            dirty = true;
            match event::read()? {
                Event::Key(key)
                    if !session.release_events
                        && matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) =>
                {
                    let now = Instant::now();
                    if now < quit_keys_muted_until {
                        quit_keys_muted_until = now + QUIT_KEY_COOLDOWN;
                    } else {
                        keys.push(key);
                    }
                }
                Event::Key(key) => keys.push(key),
                _ => {}
            }
        }
        gamepads.poll();
        let style = gamepads.style();
        if picker.pad() != style {
            picker.set_pad(style);
            dirty = true;
        }
        let held = gamepads.held();
        keys.extend(
            held.since(pads)
                .iter()
                .filter_map(|b| library_pad_key(b, style.unwrap_or(PadStyle::Generic)))
                .map(|code| KeyEvent::new(code, KeyModifiers::NONE)),
        );
        pads = held;
        let Some(outcome) = keys.into_iter().find_map(|key| {
            dirty = true;
            picker.handle(key)
        }) else {
            continue;
        };
        match outcome {
            Outcome::Quit => return Ok(()),
            Outcome::Play(rom) => {
                picker.mark_played(&rom);
                match play(terminal, &rom, session, gamepads) {
                    Ok(GameExit::Back) => {}
                    Ok(GameExit::Quit) => return Ok(()),
                    // A broken ROM, or a bug it trips over, should not
                    // take the library down with it.
                    Err(err @ (AppError::Core(_) | AppError::Patch { .. })) => {
                        picker.notify_error(err.to_string());
                    }
                    Err(err @ AppError::Crash(_)) => {
                        picker.notify_error(format!("{err} (see {})", crash_log_hint()));
                    }
                    Err(err) => return Err(err),
                }
                quit_keys_muted_until = Instant::now() + QUIT_KEY_COOLDOWN;
                pads = gamepads.held();
                dirty = true;
                // No explicit clear: the next draw diffs against the game's
                // last frame and repaints every cell that differs. (Ratatui's
                // `clear` also queries the cursor position, which some
                // terminals never answer, killing the loop with an I/O error.)
                picker.rescan();
            }
        }
    }
}

/// How long the library waits for the terminal before reading the pads.
const LIBRARY_PAD_POLL: Duration = Duration::from_millis(16);

/// What a pad button does in the library, as the key that does the same:
/// the D-pad (or stick) moves, the shoulders page, the pad's confirm
/// button or Start plays. Nothing on a pad leaves or quits; that stays
/// on the keyboard.
fn library_pad_key(button: PadButton, style: PadStyle) -> Option<KeyCode> {
    Some(match button {
        PadButton::Up => KeyCode::Up,
        PadButton::Down => KeyCode::Down,
        PadButton::L1 => KeyCode::PageUp,
        PadButton::R1 => KeyCode::PageDown,
        PadButton::Start => KeyCode::Enter,
        b if b == style.confirm() => KeyCode::Enter,
        _ => return None,
    })
}

/// Where to point the user for details of a crash.
fn crash_log_hint() -> String {
    crashlog::path().map_or_else(
        || "crash log unavailable".to_string(),
        |p| library::compact_home(&p),
    )
}

/// Loads a cartridge and its save file, applying the patch next to it
/// if there is one. Returns the patch's path along with the machine.
fn load_gba(rom: &Path) -> Result<(Gba, Option<PathBuf>), AppError> {
    let mut image = std::fs::read(rom).map_err(tuiba_core::GbaError::RomIo)?;
    let patch = find_patch(rom);
    if let Some(path) = &patch {
        let failed = |err: tuiba_core::GbaError| AppError::Patch {
            name: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            err,
        };
        let bytes = std::fs::read(path).map_err(|e| failed(tuiba_core::GbaError::RomIo(e)))?;
        image = tuiba_core::patch::apply(&image, &bytes).map_err(failed)?;
    }
    let mut gba = Gba::new(Cartridge::from_bytes(image)?);
    if let Ok(data) = std::fs::read(rom.with_extension("sav")) {
        gba.load_save_data(&data);
    }
    Ok((gba, patch))
}

/// The patch that goes with `rom`: a file of the same name with a
/// `.bps`, `.ups` or `.ips` extension, tried in that order (the formats
/// with checksums first, should a folder somehow hold more than one).
fn find_patch(rom: &Path) -> Option<PathBuf> {
    [Format::Bps, Format::Ups, Format::Ips]
        .into_iter()
        .map(|format| rom.with_extension(format.extension()))
        .find(|path| path.is_file())
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
    gamepads: &mut Gamepads,
) -> Result<GameExit, AppError> {
    let (gba, patch) = load_gba(rom)?;
    let header_title = gba.bus.cartridge.header().title.trim().to_string();
    let title = if library::is_placeholder_title(&header_title) {
        // Untitled homebrew: fall back to the file name, as the library
        // does.
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
        keypad: Keypad::new(session.release_events),
        graphics: session.renderer.protocol().map(Graphics::new),
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
        rewind_key: Hold::new(session.release_events),
        rewind: Rewind::default(),
        rewinding: false,
        pads: PadSet::EMPTY,
        pads_ignored: PadSet::EMPTY,
        pad_style: None,
        bound_leave: None,
        bindings: session.bindings.clone(),
        help: false,
        leave_armed: None,
        esc_released: false,
        seen_release: false,
        // Opened even when starting muted, so M can turn the sound on.
        audio: AudioOutput::open().map_err(|e| e.to_string()),
        muted: !session.sound,
        volume: session.volume.get(),
        stats: session.stats.get().then(|| Stats::new(Instant::now())),
        fast_speed: session.fast_speed.get(),
        rom: rom.to_path_buf(),
        slot: 0,
        held_state: None,
        panel: None,
        state_notice: patch.map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            (format!("patched with {name}"), Instant::now())
        }),
    };
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // Whatever the pads did in the library is old news, and a button
        // still held from there is not a fresh press.
        gamepads.poll();
        app.pads = gamepads.held();
        app.pads_ignored = app.pads;
        event_loop(terminal, &mut app, gamepads)
    }));

    // Persist the save however the loop ended.
    app.flush_save();
    session.volume.set(app.volume);
    session.stats.set(app.stats.is_some());
    session.fast_speed.set(app.fast_speed);
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

/// One key in a game: the fixed keys, the panel while it is up, then the
/// bindings. Returns how the game ended, when the key ends it.
/// Whether an Esc press is the second of two, and so leaves the game.
///
/// `since_first` is how long ago the previous Esc arrived, if one did.
/// A press counts as a second one when it is inside [`LEAVE_WINDOW`]
/// and either the first has been let go of, in a terminal that has
/// shown it reports releases, or — where no release will ever come —
/// far enough from the first not to be that same press reaching us
/// twice.
fn esc_leaves_game(since_first: Option<Duration>, esc_released: bool, seen_release: bool) -> bool {
    let Some(since_first) = since_first else {
        return false;
    };
    if since_first >= LEAVE_WINDOW {
        return false;
    }
    if seen_release {
        // The terminal has shown it reports releases, so the honest
        // question is whether the first Esc is over — however fast the
        // two taps came.
        esc_released
    } else {
        // No release will come; distance in time is all there is.
        since_first >= MIN_LEAVE_GAP
    }
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) -> Option<GameExit> {
    let now = Instant::now();
    // Ctrl+Q is fixed, and reaches even the panel: no overlay should be
    // able to stand between the player and the way out.
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(GameExit::Quit);
    }
    // The panel has the keyboard while it is up, Esc included.
    if app.panel.is_some() {
        app.handle_panel_key(key, now);
        return None;
    }
    if key.kind == KeyEventKind::Release {
        app.seen_release = true;
        if key.code == KeyCode::Esc {
            app.esc_released = true;
            return None;
        }
    }
    if key.kind == KeyEventKind::Press {
        match key.code {
            KeyCode::Esc if app.help => {
                app.toggle_help(now);
                return None;
            }
            // Twice within the window: one Esc is too easy to hit by
            // accident to throw a game away on. The second press only
            // counts once the first has been let go of — or, where the
            // terminal does not say so, once enough time has passed that
            // it cannot be the same press arriving twice.
            KeyCode::Esc => {
                if esc_leaves_game(
                    app.leave_armed.map(|t| now.duration_since(t)),
                    app.esc_released,
                    app.seen_release,
                ) {
                    return Some(GameExit::Back);
                }
                app.leave_armed = Some(now);
                app.esc_released = false;
                return None;
            }
            KeyCode::Char('?') => {
                app.toggle_help(now);
                return None;
            }
            _ => {}
        }
    }
    // Chords like Ctrl+Z stay with the terminal.
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match app.bindings.action(key.code) {
        Some(Action::Button(button)) => app.keypad.handle(button, key.kind, now),
        Some(Action::FastForward) => app.fast.update(key.kind, now),
        Some(Action::Rewind) => app.rewind_key.update(key.kind, now),
        Some(action) if key.kind == KeyEventKind::Press => return app.trigger(action, now),
        _ => {}
    }
    None
}

/// Emulate → draw → handle input, paced to the GBA's frame rate.
///
/// The game keeps real time even where drawing cannot: a terminal slow to
/// take a frame (sixel in Windows Terminal, a busy machine) makes us late,
/// and the next pass emulates the frames it missed without drawing them,
/// up to [`MAX_CATCH_UP`] at once. The picture then skips frames rather
/// than the whole game slowing down — and the sound, which is made by
/// emulated frames, keeps coming at the rate the device plays it.
/// Fast-forward keeps drawing at the usual rate but fills each period
/// with as many frames as the machine manages; pause keeps the loop (and
/// input) alive with no emulation at all.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    gamepads: &mut Gamepads,
) -> Result<GameExit, AppError> {
    let mut next_frame = Instant::now();
    let mut next_autosave = next_frame + AUTOSAVE_INTERVAL;
    loop {
        let now = Instant::now();
        if let Some(exit) = app.poll_pads(gamepads, now) {
            return Ok(exit);
        }
        let rewinding = !app.help && app.panel.is_none() && app.rewind_held(now);
        if app.help || app.panel.is_some() {
            // An overlay covers the screen; nothing to see, so nothing to run.
        } else if rewinding {
            // Paused or not: stepping back while paused is how to look
            // for the frame just before a mistake.
            app.rewind_step(now);
        } else if app.paused {
            if app.step {
                app.emulate_frame(now);
                app.rewind.frame(&app.gba);
                app.step = false;
            }
        } else if app.fast_held(now) {
            // As many frames as fit in the period, or as the limit
            // allows; with a limit the loop's usual sleep paces the rest.
            let deadline = now + FRAME_PERIOD;
            let cap = app.fast_speed.cap().unwrap_or(u32::MAX);
            for _ in 0..cap {
                let now = Instant::now();
                app.emulate_frame(now);
                if now >= deadline {
                    break;
                }
            }
            // Captured per frame drawn, not per frame emulated: the
            // rewind then retraces what was seen, and fast-forward does
            // not pay for states nobody watched.
            app.rewind.frame(&app.gba);
        } else {
            let due = frames_due(next_frame, now);
            for _ in 0..due {
                app.emulate_frame(Instant::now());
            }
            // The frames just caught up on were never drawn.
            app.rewind.frame(&app.gba);
            next_frame += FRAME_PERIOD * (due - 1);
        }
        app.rewinding = rewinding;
        if now >= next_autosave {
            app.flush_save();
            next_autosave = now + AUTOSAVE_INTERVAL;
        }
        let overlay = app.help || app.panel.is_some();
        let draw_start = Instant::now();
        if overlay && let Some(graphics) = &mut app.graphics {
            // The image would sit on top of the overlay (or, painted into
            // the cells, stay around it). Before the draw, so the overlay
            // lands on blank cells.
            graphics.hide()?;
        }
        terminal.draw(|frame| app.draw(frame))?;
        if !overlay && let Some(graphics) = &mut app.graphics {
            graphics.present(app.gba.framebuffer(), app.screen_area)?;
        }
        if let Some(stats) = &mut app.stats {
            let now = Instant::now();
            stats.draw(now - draw_start, now);
        }

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()?
                && let Some(exit) = handle_key(app, key)
            {
                return Ok(exit);
            }
        }

        next_frame += FRAME_PERIOD;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else if now - next_frame > FRAME_PERIOD * MAX_CATCH_UP {
            // Too far behind to catch up (fast-forward, an overlay, a
            // stall): start the count again from here.
            next_frame = now;
        }
    }
}

/// Most frames one pass of the loop emulates to catch up with the clock:
/// the game keeps full speed on terminals taking up to this many frame
/// periods (100 ms) to draw one. A machine that cannot even emulate at
/// full speed would, without a limit, spend ever longer passes catching
/// up and never draw or read input.
const MAX_CATCH_UP: u32 = 6;

/// How many frames are due at `now` when the next one was due at `next`:
/// one, plus those whole periods we are late by, up to [`MAX_CATCH_UP`].
fn frames_due(next: Instant, now: Instant) -> u32 {
    let late = now.saturating_duration_since(next);
    let missed = late.as_micros() / FRAME_PERIOD.as_micros();
    (1 + missed).min(u128::from(MAX_CATCH_UP)) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_frames_are_caught_up_within_a_bound() {
        let next = Instant::now();
        let at = |d: Duration| frames_due(next, next + d);
        assert_eq!(frames_due(next + FRAME_PERIOD, next), 1, "early");
        assert_eq!(at(Duration::ZERO), 1, "on time");
        assert_eq!(at(FRAME_PERIOD / 2), 1, "late, but not a frame late");
        assert_eq!(at(FRAME_PERIOD * 2 + FRAME_PERIOD / 2), 3);
        assert_eq!(at(Duration::from_secs(1)), MAX_CATCH_UP);
    }

    /// One physical press can reach us as several events — a terminal
    /// that sends a key's press and release as the same bare `\x1b`, or
    /// a burst from a held key. None of that may throw a game away.
    #[test]
    fn one_esc_never_leaves_the_game() {
        let no_release = |ms| esc_leaves_game(Some(Duration::from_millis(ms)), false, false);
        assert!(!esc_leaves_game(None, false, false));
        // The same press arriving twice, milliseconds apart.
        assert!(!no_release(5));
        // A Kitty asked only for event types: one press of Esc arrives as
        // two bare escapes 79 ms apart, with no release in sight.
        assert!(!no_release(79));
        // Held down where releases are reported: none has come, so
        // however long it is held, it is still one press.
        let held = esc_leaves_game(Some(Duration::from_millis(900)), false, true);
        assert!(!held);
    }

    #[test]
    fn two_presses_leave_the_game() {
        // Released in between, as a terminal with the keyboard protocol
        // reports it. A fast double tap counts: the release has already
        // proved there were two of them.
        let tapped_twice = esc_leaves_game(Some(Duration::from_millis(120)), true, true);
        assert!(tapped_twice);
        // A terminal that never sends releases: distance in time is all
        // there is to go on.
        let far_apart = esc_leaves_game(Some(Duration::from_millis(400)), false, false);
        assert!(far_apart);
    }

    #[test]
    fn the_second_press_has_to_be_soon_enough() {
        assert!(!esc_leaves_game(Some(LEAVE_WINDOW), true, true));
        assert!(!esc_leaves_game(
            Some(LEAVE_WINDOW + Duration::from_secs(1)),
            true,
            true
        ));
    }
}
