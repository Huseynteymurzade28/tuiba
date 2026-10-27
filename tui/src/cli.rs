//! Command-line argument parsing.
//!
//! Hand-rolled rather than pulling in a parser crate: the surface is a
//! ROM path plus a handful of debugging flags.

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

use chrono::NaiveDateTime;
use tuiba_core::Boot;

use crate::graphics::Renderer;
use crate::input::GbaKey;

/// Usage text printed on `--help` or a bad invocation.
pub const USAGE: &str = "\
usage: tuiba [<rom.gba> | <rom.zip> | <folder>] [options]

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
  --profile N           count where the CPU spends its cycles over the last N
                        frames and list the busiest addresses with their
                        instruction and return address (implies --frames N)
  --trace-io LIST       log every write to the I/O registers in LIST, with
                        frame, scanline, cycle and the writing instruction
                        or DMA channel; LIST is comma-separated register
                        names (DISPCNT, IME, ...), groups (lcd, sound, dma,
                        timers, serial, irq, all) or hex offsets
  --disasm [arm:|thumb:]START..END
                        after the run, disassemble memory from START to END
                        (hex; START+LENGTH works too) as THUMB, or as ARM
                        with arm:; without other options no frames run;
                        may be given several times

BIOS options (for the interactive and headless modes alike):
  --bios FILE           run the BIOS image FILE, dumped from your own console
                        (default: gba_bios.bin in the config directory, if
                        it is there; otherwise a built-in replacement)
  --no-bios             use the built-in replacement even if a BIOS is set up
  --skip-intro          with a BIOS, start the game without the boot logo

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
  --color-correction    show colours as the GBA's LCD does: darker, less
                        saturated (C toggles it in a game)
  -V, --version         show the version
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
    /// Start with LCD colour correction on.
    pub lcd_colours: bool,
    /// Which BIOS image to run.
    pub bios: BiosChoice,
    /// Where a machine with a BIOS image starts.
    pub boot: Boot,
}

/// Which BIOS image to run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BiosChoice {
    /// The one in the config directory, if there is one.
    #[default]
    Default,
    /// The one at this path.
    File(PathBuf),
    /// None: the built-in replacement.
    Builtin,
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
    /// How many of the last frames to profile, if any.
    pub profile: Option<u32>,
    /// I/O byte ranges whose writes are logged.
    pub trace_io: Vec<Range<u32>>,
    /// Memory ranges to disassemble after the run.
    pub disasm: Vec<Disasm>,
}

/// A memory range to disassemble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disasm {
    /// THUMB rather than ARM code.
    pub thumb: bool,
    /// First address.
    pub start: u32,
    /// First address past the range.
    pub end: u32,
}

impl Disasm {
    /// Parses `[arm:|thumb:]START..END` or `START+LENGTH`, in hex with an
    /// optional `0x`.
    fn parse(spec: &str) -> Option<Self> {
        let (thumb, range) = match spec.split_once(':') {
            Some(("arm", range)) => (false, range),
            Some(("thumb", range)) => (true, range),
            Some(_) => return None,
            None => (true, spec),
        };
        let hex = |text: &str| {
            let text = text.trim();
            let digits = text
                .strip_prefix("0x")
                .or_else(|| text.strip_prefix("0X"))
                .unwrap_or(text);
            u32::from_str_radix(digits, 16).ok()
        };
        let (start, end) = if let Some((start, end)) = range.split_once("..") {
            (hex(start)?, hex(end)?)
        } else {
            let (start, length) = range.split_once('+')?;
            let start = hex(start)?;
            (start, start.checked_add(hex(length)?)?)
        };
        (start < end).then_some(Self { thumb, start, end })
    }
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
    /// `--version` was requested; not really an error either.
    Version,
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
            Self::Version => write!(f, "tuiba {}", env!("CARGO_PKG_VERSION")),
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
        let mut disasm = Vec::new();
        let mut profile = None;
        let mut trace_io = Vec::new();
        let mut renderer = Renderer::Auto;
        let mut mute = false;
        let mut volume = 100;
        let mut stats = false;
        let mut lcd_colours = false;
        let mut fast_speed = FastSpeed::Max;
        let mut bios = BiosChoice::Default;
        let mut boot = Boot::Intro;

        while let Some(arg) = args.next() {
            let mut value = |flag: &'static str| {
                args.next()
                    .map(|v| (flag, v))
                    .ok_or_else(|| ArgError::MissingValue(flag.into()))
            };
            match arg.as_str() {
                "-h" | "--help" => return Err(ArgError::Help),
                "-V" | "--version" => return Err(ArgError::Version),
                "--frames" => frames = Some(parsed(value("--frames"), |v| v.parse().ok())?),
                "--screenshot" => screenshot = Some(PathBuf::from(value("--screenshot")?.1)),
                "--wav" => wav = Some(PathBuf::from(value("--wav")?.1)),
                "--no-graphics" => renderer = Renderer::HalfBlocks,
                "--renderer" => renderer = parsed(value("--renderer"), |v| v.parse().ok())?,
                "--bios" => bios = BiosChoice::File(PathBuf::from(value("--bios")?.1)),
                "--no-bios" => bios = BiosChoice::Builtin,
                "--skip-intro" => boot = Boot::Skip,
                "--mute" => mute = true,
                "--stats" => stats = true,
                "--color-correction" => lcd_colours = true,
                "--fast-speed" => fast_speed = parsed(value("--fast-speed"), FastSpeed::parse)?,
                "--volume" => {
                    volume = parsed(value("--volume"), |v| v.parse().ok().filter(|&n| n <= 100))?;
                }
                "--clock" => clock = Some(parsed(value("--clock"), crate::clock::parse)?),
                "--profile" => {
                    profile = Some(parsed(value("--profile"), |v| {
                        v.parse().ok().filter(|&n| n > 0)
                    })?);
                }
                "--trace-io" => {
                    trace_io.extend(parsed(value("--trace-io"), crate::trace::parse_watch)?);
                }
                "--disasm" => disasm.push(parsed(value("--disasm"), Disasm::parse)?),
                "--key" => keys.push(parsed(value("--key"), parse_key_hold)?),
                flag if flag.starts_with('-') && flag.len() > 1 => {
                    return Err(ArgError::Unknown(arg));
                }
                _ if rom.is_none() => rom = Some(PathBuf::from(arg)),
                _ => return Err(ArgError::Unknown(arg)),
            }
        }

        let runs = frames.is_some()
            || screenshot.is_some()
            || wav.is_some()
            || !keys.is_empty()
            || clock.is_some()
            || profile.is_some()
            || !trace_io.is_empty();
        let debug = runs || !disasm.is_empty();
        if debug && rom.is_none() {
            return Err(ArgError::MissingRom);
        }
        let headless = debug.then(|| Headless {
            // A screenshot with no frame count means "the first frame";
            // a disassembly alone looks at the machine as it boots.
            frames: frames.or(profile).unwrap_or(u32::from(runs)),
            screenshot,
            wav,
            keys,
            clock,
            profile,
            trace_io,
            disasm,
        });
        Ok(Self {
            rom,
            headless,
            renderer,
            mute,
            volume,
            stats,
            fast_speed,
            lcd_colours,
            bios,
            boot,
        })
    }
}

/// Parses a flag's value, or names the flag and value that failed.
fn parsed<T>(
    value: Result<(&str, String), ArgError>,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<T, ArgError> {
    let (flag, v) = value?;
    parse(&v).ok_or_else(|| bad(flag, &v))
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
                lcd_colours: false,
                bios: BiosChoice::Default,
                boot: Boot::Intro,
            }
        );
        assert_eq!(
            parse(&["--bios", "gba.bin"]).unwrap().bios,
            BiosChoice::File(PathBuf::from("gba.bin"))
        );
        assert_eq!(parse(&["--no-bios"]).unwrap().bios, BiosChoice::Builtin);
        assert!(parse(&["--bios"]).is_err());
        assert_eq!(parse(&["--skip-intro"]).unwrap().boot, Boot::Skip);
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
        assert!(parse(&["--color-correction"]).unwrap().lcd_colours);
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
    fn trace_io_lists_accumulate() {
        let args = parse(&["game.gba", "--trace-io", "IME", "--trace-io", "dispcnt,0x4"]).unwrap();
        let headless = args.headless.unwrap();
        assert_eq!(
            headless.trace_io,
            [0x208..0x20A, 0x000..0x002, 0x004..0x006]
        );
        assert_eq!(headless.frames, 1);
        assert!(matches!(
            parse(&["game.gba", "--trace-io", "BOGUS"]),
            Err(ArgError::BadValue { .. })
        ));
    }

    #[test]
    fn profile_window() {
        let alone = parse(&["game.gba", "--profile", "30"]).unwrap();
        let alone = alone.headless.unwrap();
        assert_eq!((alone.frames, alone.profile), (30, Some(30)));
        let tail = parse(&["game.gba", "--frames", "600", "--profile", "60"]).unwrap();
        let tail = tail.headless.unwrap();
        assert_eq!((tail.frames, tail.profile), (600, Some(60)));
        assert!(matches!(
            parse(&["game.gba", "--profile", "0"]),
            Err(ArgError::BadValue { .. })
        ));
    }

    #[test]
    fn disasm_ranges() {
        let args = parse(&[
            "game.gba",
            "--disasm",
            "arm:0x08000000..0x08000010",
            "--disasm",
            "80000c0+40",
        ])
        .unwrap();
        let headless = args.headless.unwrap();
        assert_eq!(headless.frames, 0, "a disassembly alone runs nothing");
        assert_eq!(
            headless.disasm,
            [
                Disasm {
                    thumb: false,
                    start: 0x0800_0000,
                    end: 0x0800_0010
                },
                Disasm {
                    thumb: true,
                    start: 0x0800_00C0,
                    end: 0x0800_0100
                },
            ]
        );
        let with_frames = parse(&["game.gba", "--frames", "60", "--disasm", "3000000+10"]);
        assert_eq!(with_frames.unwrap().headless.unwrap().frames, 60);
        for spec in ["x:0..4", "10..10", "zz..20", "ffffffff+2", "100"] {
            assert!(
                matches!(
                    parse(&["game.gba", "--disasm", spec]),
                    Err(ArgError::BadValue { .. })
                ),
                "{spec}"
            );
        }
    }

    #[test]
    fn errors() {
        assert_eq!(parse(&["--frames", "1"]), Err(ArgError::MissingRom));
        assert_eq!(parse(&["--help"]), Err(ArgError::Help));
        assert_eq!(parse(&["-V"]), Err(ArgError::Version));
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
