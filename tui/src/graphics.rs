//! Pixel output through the Kitty graphics protocol.
//!
//! Terminals that speak the protocol (Kitty, Ghostty, `WezTerm`, Konsole…)
//! can show the real framebuffer instead of half-block approximations.
//! The frame is upscaled by the largest integer factor that fits the
//! cell area, with nearest-neighbour sampling so pixels stay crisp, and
//! handed over either through a file in `/dev/shm` (local sessions: no
//! encoding, no pty bandwidth) or inline as base64 (works over SSH).
//!
//! Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use ratatui::layout::Rect;
use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

/// Image and placement id we reuse for every frame, so each one replaces
/// the previous instead of piling up.
const IMAGE_ID: u32 = 1;
/// Largest upscale factor: beyond this the terminal scales the rest.
const MAX_FACTOR: usize = 6;
/// Base64 payload bytes per escape sequence (the protocol's limit).
const CHUNK: usize = 4096;

/// How pixels travel to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    /// A temporary file the terminal reads and deletes.
    SharedFile,
    /// Base64 inside the escape sequence.
    Direct,
}

/// Whether the terminal advertises the protocol, judged from the
/// environment. A wrong guess is harmless: unknown escapes are ignored
/// and `--no-graphics` forces the half-block renderer. Multiplexers
/// (tmux, screen) would need the escapes wrapped, so they are excluded.
#[must_use]
pub fn terminal_supports_kitty_graphics() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let term = var("TERM");
    let program = var("TERM_PROGRAM");
    if std::env::var_os("TMUX").is_some() || term.starts_with("screen") {
        return false;
    }
    term.contains("kitty")
        || term.contains("ghostty")
        || !var("KITTY_WINDOW_ID").is_empty()
        || !var("GHOSTTY_RESOURCES_DIR").is_empty()
        || !var("WEZTERM_PANE").is_empty()
        || !var("KONSOLE_VERSION").is_empty()
        || program.eq_ignore_ascii_case("WezTerm")
        || program.eq_ignore_ascii_case("ghostty")
}

/// One image slot on the terminal.
#[derive(Debug)]
pub struct KittyGraphics {
    transport: Transport,
    /// Scratch for the upscaled RGB frame.
    pixels: Vec<u8>,
    /// Scratch for the encoded escape sequences.
    out: Vec<u8>,
    /// The one temporary file frames go through. The terminal deletes it
    /// after reading; reusing the name keeps a terminal that ignores the
    /// escapes from filling the tmpfs with orphaned frames.
    file: PathBuf,
    /// Last placement, to skip re-sending when nothing changed.
    last: Option<Placement>,
}

/// Where and how large the image was last shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    x: u16,
    y: u16,
    cols: u16,
    rows: u16,
    factor: usize,
    /// The area is smaller than the screen: the terminal scales down.
    shrink: bool,
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

impl KittyGraphics {
    /// Prepares a slot; picks the file transport when a local tmpfs is
    /// available and the session is not remote.
    #[must_use]
    pub fn new() -> Self {
        let remote =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        let shm = PathBuf::from("/dev/shm");
        let transport = if !remote && shm.is_dir() && fs::metadata(&shm).is_ok() {
            Transport::SharedFile
        } else {
            Transport::Direct
        };
        Self {
            transport,
            pixels: Vec::new(),
            out: Vec::new(),
            file: shm.join(format!("tuiba-{}", std::process::id())),
            last: None,
        }
    }

    /// Placement for `area`: the largest integer scale that fits, centred.
    #[allow(clippy::cast_precision_loss)] // screen dimensions are tiny
    fn placement(area: Rect, cell: CellSize) -> Placement {
        let area_w = f64::from(area.width) * cell.width;
        let area_h = f64::from(area.height) * cell.height;
        let fit = (area_w / SCREEN_WIDTH as f64).min(area_h / SCREEN_HEIGHT as f64);
        // Below 1:1 the terminal shrinks the native image to the cell box.
        let factor = (fit.floor() as usize).clamp(1, MAX_FACTOR);
        let (img_w, img_h) = if fit >= 1.0 {
            (
                (SCREEN_WIDTH * factor) as f64,
                (SCREEN_HEIGHT * factor) as f64,
            )
        } else {
            (SCREEN_WIDTH as f64 * fit, SCREEN_HEIGHT as f64 * fit)
        };
        let cols = ((img_w / cell.width).ceil() as u16).clamp(1, area.width.max(1));
        let rows = ((img_h / cell.height).ceil() as u16).clamp(1, area.height.max(1));
        Placement {
            x: area.x + (area.width - cols) / 2,
            y: area.y + (area.height - rows) / 2,
            cols,
            rows,
            factor: if fit >= 1.0 { factor } else { 1 },
            shrink: fit < 1.0,
        }
    }

    /// Fills `self.pixels` with the frame scaled by `factor`.
    fn upscale(&mut self, fb: &Framebuffer, factor: usize) {
        let (w, h) = (SCREEN_WIDTH * factor, SCREEN_HEIGHT * factor);
        self.pixels.clear();
        self.pixels.reserve(w * h * 3);
        for y in 0..SCREEN_HEIGHT {
            let row_start = self.pixels.len();
            for &p in fb.row(y) {
                let rgb = [(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8];
                for _ in 0..factor {
                    self.pixels.extend_from_slice(&rgb);
                }
            }
            for _ in 1..factor {
                self.pixels.extend_from_within(row_start..row_start + w * 3);
            }
        }
    }

    /// Shows `fb` inside `area`, replacing the previous frame.
    pub fn present(&mut self, fb: &Framebuffer, area: Rect) -> io::Result<()> {
        if area.width == 0 || area.height == 0 {
            return self.clear();
        }
        let placement = Self::placement(area, CellSize::query());
        self.upscale(fb, placement.factor);
        let (w, h) = (
            SCREEN_WIDTH * placement.factor,
            SCREEN_HEIGHT * placement.factor,
        );

        self.out.clear();
        // Park the cursor at the image origin; `C=1` keeps it there.
        write!(self.out, "\x1b[{};{}H", placement.y + 1, placement.x + 1)?;
        // At native size the terminal must not stretch the image to a
        // whole number of cells; only a shrink needs the cell box.
        let fit_box = if placement.shrink {
            format!(",c={},r={}", placement.cols, placement.rows)
        } else {
            String::new()
        };
        let control = format!("a=T,f=24,s={w},v={h}{fit_box},i={IMAGE_ID},p={IMAGE_ID},C=1,q=2");
        match self.transport {
            Transport::SharedFile => {
                if fs::write(&self.file, &self.pixels).is_err() {
                    // tmpfs went away: fall back for good.
                    self.transport = Transport::Direct;
                    return self.present(fb, area);
                }
                write!(self.out, "\x1b_G{control},t=t;")?;
                base64_into(self.file.as_os_str().as_encoded_bytes(), &mut self.out);
                self.out.extend_from_slice(b"\x1b\\");
            }
            Transport::Direct => {
                let mut encoded = Vec::with_capacity(self.pixels.len() / 3 * 4 + 4);
                base64_into(&self.pixels, &mut encoded);
                let chunks = encoded.chunks(CHUNK).collect::<Vec<_>>();
                for (n, chunk) in chunks.iter().enumerate() {
                    let more = u8::from(n + 1 < chunks.len());
                    if n == 0 {
                        write!(self.out, "\x1b_G{control},t=d,m={more};")?;
                    } else {
                        write!(self.out, "\x1b_Gm={more},q=2;")?;
                    }
                    self.out.extend_from_slice(chunk);
                    self.out.extend_from_slice(b"\x1b\\");
                }
            }
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(&self.out)?;
        stdout.flush()?;
        self.last = Some(placement);
        Ok(())
    }

    /// Removes the image from the screen.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.last.take().is_some() {
            let mut stdout = io::stdout().lock();
            write!(stdout, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\")?;
            stdout.flush()?;
        }
        Ok(())
    }
}

impl Drop for KittyGraphics {
    fn drop(&mut self) {
        let _ = self.clear();
        let _ = fs::remove_file(&self.file);
    }
}

/// Standard base64 with padding, appended to `out`.
fn base64_into(data: &[u8], out: &mut Vec<u8>) {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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
        let p = KittyGraphics::placement(Rect::new(0, 0, 100, 40), cell);
        assert_eq!(p.factor, 4);
        assert!(!p.shrink);
        assert_eq!((p.cols, p.rows), (96, 32));
        assert_eq!((p.x, p.y), (2, 4), "centred");

        // Too small for 1:1 (200 × 200 px): the terminal shrinks a
        // native-size image to 200 × 133 px.
        let p = KittyGraphics::placement(Rect::new(5, 5, 20, 10), cell);
        assert_eq!(p.factor, 1);
        assert!(p.shrink);
        assert_eq!((p.cols, p.rows), (20, 7));
        assert_eq!((p.x, p.y), (5, 6));

        // The factor is capped.
        let p = KittyGraphics::placement(Rect::new(0, 0, 400, 200), cell);
        assert_eq!(p.factor, MAX_FACTOR);
    }

    #[test]
    fn upscale_replicates_pixels() {
        let mut fb = Framebuffer::new();
        fb.row_mut(0)[0] = 0xFF00_00FF;
        fb.row_mut(0)[1] = 0x00FF_00FF;
        let mut g = KittyGraphics::new();
        g.upscale(&fb, 2);
        assert_eq!(g.pixels.len(), SCREEN_WIDTH * 2 * SCREEN_HEIGHT * 2 * 3);
        let row = SCREEN_WIDTH * 2 * 3;
        assert_eq!(
            &g.pixels[..12],
            &[255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0]
        );
        assert_eq!(
            &g.pixels[row..row + 12],
            &g.pixels[..12],
            "second row is a copy"
        );
        assert_eq!(
            &g.pixels[2 * row..2 * row + 3],
            &[0, 0, 0],
            "next source row"
        );
    }
}
