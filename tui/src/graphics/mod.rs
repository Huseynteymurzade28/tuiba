//! Pixel output through terminal image protocols.
//!
//! Terminals that can show images get the real framebuffer instead of
//! half-block approximations. Three protocols are spoken:
//!
//! - **Kitty** (Kitty, Ghostty, `WezTerm`, Konsole): the image lives on a
//!   layer above the text, keyed by an id, so each frame replaces the
//!   last and deleting it leaves the cells untouched.
//! - **Sixel** (foot, Windows Terminal, mintty, mlterm, xterm as a VT340):
//!   a paletted image painted into the cells themselves.
//! - **iTerm2** inline images (iTerm2, and anything that sets
//!   `LC_TERMINAL=iTerm2`): a PNG painted into the cells.
//!
//! The frame is scaled by the largest integer factor that fits the cell
//! area, with nearest-neighbour sampling so pixels stay crisp. Sixel and
//! iTerm2 images overwrite the cells they cover, so taking one away means
//! repainting those cells; see [`Graphics::hide`].

mod iterm;
mod kitty;
mod sixel;

use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;

use ratatui::layout::Rect;
use ratatui::style::Color;
use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::theme;

/// Largest upscale factor: beyond this the terminal scales the rest
/// (Kitty) or the image just stops growing.
const MAX_FACTOR: usize = 6;

/// An image protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    Sixel,
    Iterm2,
}

impl Protocol {
    /// Name for the status bar and `--renderer`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
        }
    }

    /// The protocol the terminal speaks, judged from the environment.
    /// A wrong guess is harmless: unknown escapes are ignored and
    /// `--renderer` overrides it. Multiplexers (tmux, screen) would need
    /// the escapes wrapped, so they get none.
    #[must_use]
    pub fn detect() -> Option<Self> {
        Self::detect_from(|name| std::env::var(name).unwrap_or_default())
    }

    fn detect_from(var: impl Fn(&str) -> String) -> Option<Self> {
        let term = var("TERM");
        let program = var("TERM_PROGRAM");
        let set = |name: &str| !var(name).is_empty();
        let program_is = |name: &str| program.eq_ignore_ascii_case(name);
        if set("TMUX") || term.starts_with("screen") || term.starts_with("tmux") {
            return None;
        }
        if term.contains("kitty")
            || term.contains("ghostty")
            || set("KITTY_WINDOW_ID")
            || set("GHOSTTY_RESOURCES_DIR")
            || set("WEZTERM_PANE")
            || set("KONSOLE_VERSION")
            || program_is("WezTerm")
            || program_is("ghostty")
        {
            return Some(Self::Kitty);
        }
        // iTerm2 passes LC_TERMINAL through SSH, TERM_PROGRAM it does not.
        if program_is("iTerm.app") || var("LC_TERMINAL") == "iTerm2" {
            return Some(Self::Iterm2);
        }
        if term.starts_with("foot")
            || term.starts_with("mlterm")
            || term.starts_with("contour")
            || set("WT_SESSION")
            || set("MLTERM")
            || program_is("mintty")
            || var("TERMINAL_NAME") == "contour"
        {
            return Some(Self::Sixel);
        }
        None
    }

    /// Whether the image is painted into the cells rather than onto a
    /// layer above them.
    fn in_cells(self) -> bool {
        self != Self::Kitty
    }
}

/// How the game screen is drawn, as asked on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    /// An image protocol if the terminal seems to speak one, else
    /// half-blocks.
    #[default]
    Auto,
    /// Always half-block characters.
    HalfBlocks,
    /// This protocol, whatever the terminal claims.
    Pixels(Protocol),
}

impl Renderer {
    /// The protocol to use, if any.
    #[must_use]
    pub fn protocol(self) -> Option<Protocol> {
        match self {
            Self::Auto => Protocol::detect(),
            Self::HalfBlocks => None,
            Self::Pixels(p) => Some(p),
        }
    }
}

impl FromStr for Renderer {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "blocks" | "half-blocks" | "halfblocks" => Self::HalfBlocks,
            "kitty" => Self::Pixels(Protocol::Kitty),
            "sixel" => Self::Pixels(Protocol::Sixel),
            "iterm2" | "iterm" => Self::Pixels(Protocol::Iterm2),
            _ => return Err(()),
        })
    }
}

impl fmt::Display for Renderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::HalfBlocks => "blocks",
            Self::Pixels(p) => p.name(),
        })
    }
}

/// Where and how large the image is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    x: u16,
    y: u16,
    cols: u16,
    rows: u16,
    /// Integer upscale factor (1 when shrinking).
    factor: usize,
    /// The area is smaller than the screen: the image must shrink.
    shrink: bool,
    /// Image size in pixels on the terminal: the screen times `factor`,
    /// or the shrunk size.
    width: usize,
    height: usize,
}

/// The cell grid in pixels, as far as the terminal will tell us.
#[derive(Debug, Clone, Copy)]
struct CellSize {
    width: f64,
    height: f64,
}

impl CellSize {
    /// Queries the terminal; falls back to a typical 1:2 cell when it
    /// does not report pixel dimensions.
    fn query() -> Self {
        let fallback = Self {
            width: 8.0,
            height: 16.0,
        };
        let Ok(size) = crossterm::terminal::window_size() else {
            return fallback;
        };
        if size.width == 0 || size.height == 0 || size.columns == 0 || size.rows == 0 {
            return fallback;
        }
        Self {
            width: f64::from(size.width) / f64::from(size.columns),
            height: f64::from(size.height) / f64::from(size.rows),
        }
    }
}

/// Placement for `area`: the largest integer scale that fits, centred.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // screen dimensions are tiny and positive
fn placement(area: Rect, cell: CellSize) -> Placement {
    let area_w = f64::from(area.width) * cell.width;
    let area_h = f64::from(area.height) * cell.height;
    let fit = (area_w / SCREEN_WIDTH as f64).min(area_h / SCREEN_HEIGHT as f64);
    let shrink = fit < 1.0;
    let factor = if shrink {
        1
    } else {
        (fit.floor() as usize).min(MAX_FACTOR)
    };
    let (width, height) = if shrink {
        (
            ((SCREEN_WIDTH as f64 * fit) as usize).max(1),
            ((SCREEN_HEIGHT as f64 * fit) as usize).max(1),
        )
    } else {
        (SCREEN_WIDTH * factor, SCREEN_HEIGHT * factor)
    };
    let cols = ((width as f64 / cell.width).ceil() as u16).clamp(1, area.width.max(1));
    let rows = ((height as f64 / cell.height).ceil() as u16).clamp(1, area.height.max(1));
    Placement {
        x: area.x + (area.width - cols) / 2,
        y: area.y + (area.height - rows) / 2,
        cols,
        rows,
        factor,
        shrink,
        width,
        height,
    }
}

/// The screen scaled by `factor` as packed RGB, into `pixels`.
fn upscale_rgb(fb: &Framebuffer, factor: usize, pixels: &mut Vec<u8>) {
    let w = SCREEN_WIDTH * factor;
    pixels.clear();
    pixels.reserve(w * SCREEN_HEIGHT * factor * 3);
    for y in 0..SCREEN_HEIGHT {
        let row_start = pixels.len();
        for &p in fb.row(y) {
            let rgb = [(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8];
            for _ in 0..factor {
                pixels.extend_from_slice(&rgb);
            }
        }
        for _ in 1..factor {
            pixels.extend_from_within(row_start..row_start + w * 3);
        }
    }
}

/// An encoder for one protocol.
#[derive(Debug)]
enum Backend {
    Kitty(kitty::Kitty),
    Sixel(sixel::Sixel),
    Iterm2(iterm::Iterm2),
}

/// What is on the terminal right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    /// The area it was asked to fill.
    area: Rect,
    placement: Placement,
}

/// The game screen as an image on the terminal.
#[derive(Debug)]
pub struct Graphics {
    protocol: Protocol,
    backend: Backend,
    /// Scratch for the escape sequences.
    out: Vec<u8>,
    /// The frame last sent, to skip sending the same picture again.
    frame: Vec<u32>,
    shown: Option<Shown>,
}

impl Graphics {
    #[must_use]
    pub fn new(protocol: Protocol) -> Self {
        let backend = match protocol {
            Protocol::Kitty => Backend::Kitty(kitty::Kitty::new()),
            Protocol::Sixel => Backend::Sixel(sixel::Sixel::default()),
            Protocol::Iterm2 => Backend::Iterm2(iterm::Iterm2::default()),
        };
        Self {
            protocol,
            backend,
            out: Vec::new(),
            frame: Vec::new(),
            shown: None,
        }
    }

    #[must_use]
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Shows `fb` inside `area`, replacing the previous frame. Nothing is
    /// sent when the same picture is already there: a paused game costs
    /// no bandwidth.
    pub fn present(&mut self, fb: &Framebuffer, area: Rect) -> io::Result<()> {
        if area.width == 0 || area.height == 0 {
            return self.hide();
        }
        let placement = placement(area, CellSize::query());
        let shown = Shown { area, placement };
        if self.shown == Some(shown) && self.frame == fb.pixels() {
            return Ok(());
        }

        self.out.clear();
        if let Some(old) = self.shown
            && old.area == area
            && old.placement != placement
            && self.protocol.in_cells()
        {
            // Same area, different image size (the font changed): wipe
            // the old one first. After a resize Ratatui has already
            // cleared the screen, and the old area may now hold the
            // status bar.
            erase(&mut self.out, old.placement)?;
        }
        // Park the cursor at the image origin.
        write!(self.out, "\x1b[{};{}H", placement.y + 1, placement.x + 1)?;
        match &mut self.backend {
            Backend::Kitty(k) => k.encode(fb, placement, &mut self.out)?,
            Backend::Sixel(s) => s.encode(fb, placement, &mut self.out)?,
            Backend::Iterm2(i) => i.encode(fb, placement, &mut self.out)?,
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(&self.out)?;
        stdout.flush()?;
        self.frame.clear();
        self.frame.extend_from_slice(fb.pixels());
        self.shown = Some(shown);
        Ok(())
    }

    /// Takes the image off the screen. Call it *before* drawing whatever
    /// goes in its place: for protocols that paint into the cells it
    /// repaints them blank, as Ratatui believes them to be, so Ratatui's
    /// next diff draws over a screen that matches its buffer.
    pub fn hide(&mut self) -> io::Result<()> {
        let Some(shown) = self.shown.take() else {
            return Ok(());
        };
        self.out.clear();
        match &mut self.backend {
            Backend::Kitty(_) => kitty::delete(&mut self.out)?,
            Backend::Sixel(_) | Backend::Iterm2(_) => erase(&mut self.out, shown.placement)?,
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(&self.out)?;
        stdout.flush()
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        // Leaves the cells blank for whatever Ratatui draws next.
        let _ = self.hide();
    }
}

/// Paints the cells under `p` with spaces in the screen's colours: what
/// the game screen's cells hold in Ratatui's buffer.
fn erase(out: &mut Vec<u8>, p: Placement) -> io::Result<()> {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(br, bg, bb)) = (theme::TEXT, theme::BG) else {
        unreachable!("theme colours are RGB");
    };
    write!(out, "\x1b[0;38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m")?;
    for row in 0..p.rows {
        write!(out, "\x1b[{};{}H", p.y + row + 1, p.x + 1)?;
        out.resize(out.len() + usize::from(p.cols), b' ');
    }
    out.extend_from_slice(b"\x1b[0m");
    Ok(())
}

/// Standard base64 with padding, appended to `out`.
fn base64_into(data: &[u8], out: &mut Vec<u8>) {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    out.reserve(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63]);
        out.push(TABLE[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63]
        } else {
            b'='
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        let enc = |s: &str| {
            let mut out = Vec::new();
            base64_into(s.as_bytes(), &mut out);
            String::from_utf8(out).unwrap()
        };
        assert_eq!(enc(""), "");
        assert_eq!(enc("f"), "Zg==");
        assert_eq!(enc("fo"), "Zm8=");
        assert_eq!(enc("foo"), "Zm9v");
        assert_eq!(enc("foobar"), "Zm9vYmFy");
    }

    #[test]
    fn placement_picks_the_largest_integer_scale_that_fits() {
        let cell = CellSize {
            width: 10.0,
            height: 20.0,
        };
        // 100 cols × 40 rows = 1000 × 800 px: 4× would be 960 × 640.
        let p = placement(Rect::new(0, 0, 100, 40), cell);
        assert_eq!(p.factor, 4);
        assert!(!p.shrink);
        assert_eq!((p.width, p.height), (960, 640));
        assert_eq!((p.cols, p.rows), (96, 32));
        assert_eq!((p.x, p.y), (2, 4), "centred");

        // Too small for 1:1 (200 × 200 px): shrunk to 200 × 133 px.
        let p = placement(Rect::new(5, 5, 20, 10), cell);
        assert_eq!(p.factor, 1);
        assert!(p.shrink);
        assert_eq!((p.width, p.height), (200, 133));
        assert_eq!((p.cols, p.rows), (20, 7));
        assert_eq!((p.x, p.y), (5, 6));

        // The factor is capped.
        let p = placement(Rect::new(0, 0, 400, 200), cell);
        assert_eq!(p.factor, MAX_FACTOR);
    }

    #[test]
    fn upscale_replicates_pixels() {
        let mut fb = Framebuffer::new();
        fb.row_mut(0)[0] = 0xFF00_00FF;
        fb.row_mut(0)[1] = 0x00FF_00FF;
        let mut pixels = Vec::new();
        upscale_rgb(&fb, 2, &mut pixels);
        assert_eq!(pixels.len(), SCREEN_WIDTH * 2 * SCREEN_HEIGHT * 2 * 3);
        let row = SCREEN_WIDTH * 2 * 3;
        assert_eq!(&pixels[..12], &[255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0]);
        assert_eq!(
            &pixels[row..row + 12],
            &pixels[..12],
            "second row is a copy"
        );
        assert_eq!(&pixels[2 * row..2 * row + 3], &[0, 0, 0], "next source row");
    }

    #[test]
    fn detects_protocols_from_the_environment() {
        let detect = |vars: &[(&str, &str)]| {
            let vars = vars.to_vec();
            Protocol::detect_from(move |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
                    .unwrap_or_default()
            })
        };
        assert_eq!(detect(&[("TERM", "xterm-kitty")]), Some(Protocol::Kitty));
        assert_eq!(detect(&[("TERM", "foot")]), Some(Protocol::Sixel));
        assert_eq!(detect(&[("TERM", "foot-extra")]), Some(Protocol::Sixel));
        assert_eq!(
            detect(&[("TERM", "xterm-256color"), ("WT_SESSION", "abc")]),
            Some(Protocol::Sixel)
        );
        assert_eq!(
            detect(&[("TERM_PROGRAM", "iTerm.app")]),
            Some(Protocol::Iterm2)
        );
        assert_eq!(
            detect(&[("TERM", "xterm-256color"), ("LC_TERMINAL", "iTerm2")]),
            Some(Protocol::Iterm2)
        );
        assert_eq!(detect(&[("TERM", "xterm-256color")]), None);
        assert_eq!(
            detect(&[("TERM", "foot"), ("TMUX", "/tmp/tmux-1000/default,1,0")]),
            None,
            "multiplexers get no images"
        );
    }

    #[test]
    fn renderer_names_round_trip() {
        for r in [
            Renderer::Auto,
            Renderer::HalfBlocks,
            Renderer::Pixels(Protocol::Kitty),
            Renderer::Pixels(Protocol::Sixel),
            Renderer::Pixels(Protocol::Iterm2),
        ] {
            assert_eq!(r.to_string().parse(), Ok(r));
        }
        assert_eq!("SIXEL".parse(), Ok(Renderer::Pixels(Protocol::Sixel)));
        assert!("png".parse::<Renderer>().is_err());
    }
}
