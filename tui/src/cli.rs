//! Command-line argument parsing.
//!
//! Hand-rolled rather than pulling in a parser crate: the surface is a
//! ROM path plus a handful of debugging flags.

use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::graphics::Renderer;
use crate::input::GbaKey;

/// Usage text printed on `--help` or a bad invocation.
pub const USAGE: &str = "\
usage: tuiba [<rom.gba> | <folder>] [options]

Run a Game Boy Advance ROM in the terminal. With no argument, or with a
folder, open the library screen (the folder is added to the library).

Debug options (headless, no terminal UI; need a ROM path):
  --frames N            emulate N frames and exit; the summary line ends
                        with a hash of the final frame (frame=...)
  --screenshot FILE     write the final frame as a PNG (implies --frames)
  --wav FILE            write the sound produced during the run as a WAV
                        file (implies --frames)
  --key BUTTON@FROM-TO  hold BUTTON from frame FROM to frame TO (exclusive);
                        BUTTON is one of a b select start right left up down r l;
                        may be given several times
  --clock DATETIME      what a cartridge's real-time clock reads on the first
                        frame, as YYYY-MM-DDTHH:MM:SS (default 2000-01-01
                        00:00:00); it advances with the emulated frames

Display and sound options:
  --renderer NAME       how to draw the game screen: auto (default: an image
                        protocol if the terminal seems to speak one), kitty,
                        sixel, iterm2, or blocks (half-block characters)
  --no-graphics         same as --renderer blocks
  --mute                start with sound off (M toggles it in a game)
  --volume PERCENT      sound volume, 0 to 100 (default 100; - and + change
                        it in a game)
  --fast-speed N        fast-forward at most N times real speed (2 to 16), or
                        max (default) for as fast as it goes; F4 cycles ×2,
                        ×4 and max in a game
  --stats               show frame rates, draw times and the sound queue in
                        the status bar (F3 toggles it in a game)
  -h, --help            show this help";

/// How fast fast-forward runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FastSpeed {
    /// At most this many times real speed.
    Times(u32),
    /// As fast as the machine manages.
    #[default]
    Max,
}

impl FastSpeed {
    /// The order `F4` cycles through.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Times(2) => Self::Times(4),
            Self::Times(_) => Self::Max,
            Self::Max => Self::Times(2),
        }
    }

    /// How many frames to emulate per frame drawn, `None` for no limit.
    #[must_use]
    pub const fn cap(self) -> Option<u32> {
        match self {
            Self::Times(n) => Some(n),
            Self::Max => None,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "max" => Some(Self::Max),
            _ => value
                .parse()
                .ok()
                .filter(|n| (2..=16).contains(n))
                .map(Self::Times),
        }
    }
}

impl fmt::Display for FastSpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Times(n) => write!(f, "×{n}"),
            Self::Max => f.write_str("uncapped"),
        }
    }
}

/// What the user asked us to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    /// The cartridge to load, or a folder for the library. `None` opens
    /// the library screen.
    pub rom: Option<PathBuf>,
    /// Headless run parameters, when any debug flag was given.
    pub headless: Option<Headless>,
    /// How to draw the game screen.
    pub renderer: Renderer,
    /// Start with sound off.
    pub mute: bool,
    /// Sound volume in percent, 0 to 100.
    pub volume: u8,
    /// Start with performance figures in the status bar.
    pub stats: bool,
    /// Fast-forward limit.
    pub fast_speed: FastSpeed,
}

/// Headless (non-interactive) run configuration.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Headless {
    /// Frames to emulate before exiting.
    pub frames: u32,
    /// Where to write the final frame, if anywhere.
    pub screenshot: Option<PathBuf>,
    /// Where to write the audio, if anywhere.
    pub wav: Option<PathBuf>,
    /// Scripted key holds.
    pub keys: Vec<KeyHold>,
    /// What the cartridge clock reads on the first frame; it then
    /// advances with the emulated frames. `None` starts it at
    /// [`headless::DEFAULT_CLOCK`](crate::headless::DEFAULT_CLOCK), so
    /// runs are reproducible either way.
    pub clock: Option<NaiveDateTime>,
}

/// A button held over a half-open range of frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHold {
    /// The button to press.
    pub key: GbaKey,
    /// First frame during which the button reads as pressed.
    pub from: u32,
    /// First frame during which the button reads as released again.
    pub to: u32,
}

/// Why the command line could not be parsed.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// `--help` was requested; not really an error.
    Help,
    /// A headless flag was given without a ROM path.
    MissingRom,
    /// A flag that needs a value was given without one.
    MissingValue(String),
    /// A value could not be parsed.
    BadValue {
        /// The flag whose value was rejected.
        flag: String,
        /// The offending text.
        value: String,
    },
    /// An option we do not know.
    Unknown(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Help => f.write_str(USAGE),
            Self::MissingRom => write!(f, "headless options need a ROM path\n\n{USAGE}"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value\n\n{USAGE}"),
            Self::BadValue { flag, value } => write!(f, "invalid value for {flag}: {value:?}"),
            Self::Unknown(arg) => write!(f, "unknown option {arg}\n\n{USAGE}"),
        }
    }
}

impl std::error::Error for ArgError {}

impl Args {
    /// Parses the process arguments (excluding the program name).
    pub fn parse<I>(args: I) -> Result<Self, ArgError>
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        let mut args = args.into_iter().map(Into::into);
        let mut rom = None;
        let mut frames = None;
        let mut screenshot = None;
        let mut wav = None;
        let mut keys = Vec::new();
        let mut clock = None;
        let mut renderer = Renderer::Auto;
        let mut mute = false;
        let mut volume = 100;
        let mut stats = false;
        let mut fast_speed = FastSpeed::Max;

        while let Some(arg) = args.next() {
            let mut value = |flag: &str| {
                args.next()
                    .ok_or_else(|| ArgError::MissingValue(flag.into()))
            };
            match arg.as_str() {
                "-h" | "--help" => return Err(ArgError::Help),
                "--frames" => {
                    let v = value("--frames")?;
                    frames = Some(v.parse().map_err(|_| bad("--frames", &v))?);
                }
                "--screenshot" => screenshot = Some(PathBuf::from(value("--screenshot")?)),
                "--wav" => wav = Some(PathBuf::from(value("--wav")?)),
                "--no-graphics" => renderer = Renderer::HalfBlocks,
                "--renderer" => {
                    let v = value("--renderer")?;
                    renderer = v.parse().map_err(|()| bad("--renderer", &v))?;
                }
                "--mute" => mute = true,
                "--stats" => stats = true,
                "--fast-speed" => {
                    let v = value("--fast-speed")?;
                    fast_speed = FastSpeed::parse(&v).ok_or_else(|| bad("--fast-speed", &v))?;
                }
                "--volume" => {
                    let v = value("--volume")?;
                    volume = v
                        .parse()
                        .ok()
                        .filter(|&n| n <= 100)
                        .ok_or_else(|| bad("--volume", &v))?;
                }
                "--clock" => {
                    let v = value("--clock")?;
                    clock = Some(crate::clock::parse(&v).ok_or_else(|| bad("--clock", &v))?);
                }
                "--key" => {
                    let v = value("--key")?;
                    keys.push(parse_key_hold(&v).ok_or_else(|| bad("--key", &v))?);
                }
                flag if flag.starts_with('-') && flag.len() > 1 => {
                    return Err(ArgError::Unknown(arg));
                }
                _ if rom.is_none() => rom = Some(PathBuf::from(arg)),
                _ => return Err(ArgError::Unknown(arg)),
            }
        }

        let debug = frames.is_some()
            || screenshot.is_some()
            || wav.is_some()
            || !keys.is_empty()
            || clock.is_some();
        if debug && rom.is_none() {
            return Err(ArgError::MissingRom);
        }
        let headless = debug.then(|| Headless {
            // A screenshot with no frame count means "the first frame".
            frames: frames.unwrap_or(1),
            screenshot,
            wav,
            keys,
            clock,
        });
        Ok(Self {
            rom,
            headless,
            renderer,
            mute,
            volume,
            stats,
            fast_speed,
        })
    }
}

fn bad(flag: &str, value: &str) -> ArgError {
    ArgError::BadValue {
        flag: flag.into(),
        value: value.into(),
    }
}

/// Parses `button@from-to`; `to` defaults to `from + 1` (a single-frame tap).
fn parse_key_hold(spec: &str) -> Option<KeyHold> {
    let (name, range) = spec.split_once('@')?;
    let key = parse_key(name)?;
    let (from, to) = if let Some((from, to)) = range.split_once('-') {
        (from.parse().ok()?, to.parse().ok()?)
    } else {
        let from: u32 = range.parse().ok()?;
        (from, from + 1)
    };
    (from < to).then_some(KeyHold { key, from, to })
}

fn parse_key(name: &str) -> Option<GbaKey> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a" => GbaKey::A,
        "b" => GbaKey::B,
        "select" => GbaKey::Select,
        "start" => GbaKey::Start,
        "right" => GbaKey::Right,
        "left" => GbaKey::Left,
        "up" => GbaKey::Up,
        "down" => GbaKey::Down,
        "r" => GbaKey::R,
        "l" => GbaKey::L,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::Protocol;

    fn parse(args: &[&str]) -> Result<Args, ArgError> {
        Args::parse(args.iter().copied())
    }

    #[test]
    fn rom_only_is_interactive() {
        let args = parse(&["game.gba"]).unwrap();
        assert_eq!(args.rom, Some(PathBuf::from("game.gba")));
        assert_eq!(args.headless, None);
        assert_eq!(
            parse(&[]).unwrap(),
            Args {
                rom: None,
                headless: None,
                renderer: Renderer::Auto,
                mute: false,
                volume: 100,
                stats: false,
                fast_speed: FastSpeed::Max,
            }
        );
        assert_eq!(
            parse(&["--no-graphics"]).unwrap().renderer,
            Renderer::HalfBlocks
        );
        assert_eq!(
            parse(&["--renderer", "sixel"]).unwrap().renderer,
            Renderer::Pixels(Protocol::Sixel)
        );
        assert!(parse(&["--renderer", "png"]).is_err());
        assert!(parse(&["--mute"]).unwrap().mute);
        assert_eq!(parse(&["--volume", "40"]).unwrap().volume, 40);
        assert!(parse(&["--volume", "101"]).is_err());
        assert!(parse(&["--volume", "-5"]).is_err());
        assert!(parse(&["--stats"]).unwrap().stats);
        assert_eq!(
            parse(&["--fast-speed", "4"]).unwrap().fast_speed,
            FastSpeed::Times(4)
        );
        assert_eq!(
            parse(&["--fast-speed", "max"]).unwrap().fast_speed,
            FastSpeed::Max
        );
        assert!(parse(&["--fast-speed", "1"]).is_err());
        assert!(parse(&["--fast-speed", "fast"]).is_err());
    }

    #[test]
    fn fast_speed_cycles_through_the_common_caps() {
        let mut speed = FastSpeed::Max;
        let mut seen = Vec::new();
        for _ in 0..4 {
            speed = speed.next();
            seen.push(speed.to_string());
        }
        assert_eq!(seen, ["×2", "×4", "uncapped", "×2"]);
        // A cap from the command line that is not in the cycle joins it.
        assert_eq!(FastSpeed::Times(8).next(), FastSpeed::Max);
    }

    #[test]
    fn debug_flags_select_headless_mode() {
        let args = parse(&[
            "game.gba",
            "--frames",
            "300",
            "--screenshot",
            "out.png",
            "--wav",
            "out.wav",
            "--key",
            "start@120-130",
            "--key",
            "A@200",
        ])
        .unwrap();
        let headless = args.headless.unwrap();
        assert_eq!(headless.frames, 300);
        assert_eq!(headless.screenshot, Some(PathBuf::from("out.png")));
        assert_eq!(headless.wav, Some(PathBuf::from("out.wav")));
        assert_eq!(
            headless.keys,
            vec![
                KeyHold {
                    key: GbaKey::Start,
                    from: 120,
                    to: 130
                },
                KeyHold {
                    key: GbaKey::A,
                    from: 200,
                    to: 201
                },
            ]
        );
    }

    #[test]
    fn screenshot_alone_runs_one_frame() {
        let args = parse(&["--screenshot", "x.png", "game.gba"]).unwrap();
        assert_eq!(args.headless.unwrap().frames, 1);
    }

    #[test]
    fn errors() {
        assert_eq!(parse(&["--frames", "1"]), Err(ArgError::MissingRom));
        assert_eq!(parse(&["--help"]), Err(ArgError::Help));
        assert_eq!(
            parse(&["game.gba", "--frames"]),
            Err(ArgError::MissingValue("--frames".into()))
        );
        assert!(matches!(
            parse(&["game.gba", "--frames", "lots"]),
            Err(ArgError::BadValue { .. })
        ));
        assert!(matches!(
            parse(&["game.gba", "--key", "start@10-5"]),
            Err(ArgError::BadValue { .. })
        ));
        assert!(matches!(
            parse(&["game.gba", "--key", "x@1"]),
            Err(ArgError::BadValue { .. })
        ));
        assert_eq!(
            parse(&["game.gba", "--bogus"]),
            Err(ArgError::Unknown("--bogus".into()))
        );
        assert_eq!(
            parse(&["a.gba", "b.gba"]),
            Err(ArgError::Unknown("b.gba".into()))
        );
    }
}
