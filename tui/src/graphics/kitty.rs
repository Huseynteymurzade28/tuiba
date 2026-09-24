//! The Kitty graphics protocol.
//!
//! Pixels travel either through a file in `/dev/shm` (local sessions: no
//! encoding, no pty bandwidth) or inline as base64 (works over SSH).
//!
//! Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use tuiba_core::Framebuffer;

use super::{Placement, base64_into, upscale_rgb};

/// Image and placement id we reuse for every frame, so each one replaces
/// the previous instead of piling up.
const IMAGE_ID: u32 = 1;
/// Base64 payload bytes per escape sequence (the protocol's limit).
const CHUNK: usize = 4096;
/// Frame files kept before the oldest is unlinked, for terminals that
/// take the file transport but are slow to consume (or never delete)
/// the files.
const FILE_BACKLOG: usize = 8;

/// How pixels travel to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    /// A temporary file the terminal reads and deletes.
    SharedFile,
    /// Base64 inside the escape sequence.
    Direct,
}

/// Writes the escape that removes our image.
pub(super) fn delete(out: &mut Vec<u8>) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\")
}

/// One image slot on the terminal.
#[derive(Debug)]
pub(super) struct Kitty {
    transport: Transport,
    /// Scratch for the upscaled RGB frame.
    pixels: Vec<u8>,
    /// Directory the frame files go in.
    dir: PathBuf,
    /// Number of the next frame file. Every frame gets a fresh file:
    /// the terminal maps the file to read it, and truncating one it is
    /// still reading kills the terminal with SIGBUS.
    sequence: u64,
    /// Frame files not yet unlinked by us. The terminal normally deletes
    /// each after reading, but one that ignores the escapes would leave
    /// them to pile up on the tmpfs.
    recent: VecDeque<PathBuf>,
}

impl Kitty {
    /// Picks the file transport when a local tmpfs is available and the
    /// session is not remote.
    pub(super) fn new() -> Self {
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
            dir: shm,
            sequence: 0,
            recent: VecDeque::with_capacity(FILE_BACKLOG + 1),
        }
    }

    /// Writes the frame to a new file and returns its path, unlinking
    /// the oldest one still on our books.
    fn write_frame_file(&mut self) -> io::Result<PathBuf> {
        let path = self
            .dir
            .join(format!("tuiba-{}-{}", std::process::id(), self.sequence));
        self.sequence += 1;
        fs::write(&path, &self.pixels)?;
        self.recent.push_back(path.clone());
        if self.recent.len() > FILE_BACKLOG
            && let Some(old) = self.recent.pop_front()
        {
            // Unlinking is safe even mid-read: only truncation is not.
            let _ = fs::remove_file(old);
        }
        Ok(path)
    }

    /// Appends the escapes that show `fb` at the cursor.
    pub(super) fn encode(
        &mut self,
        fb: &Framebuffer,
        placement: Placement,
        out: &mut Vec<u8>,
    ) -> io::Result<()> {
        upscale_rgb(fb, placement.factor, &mut self.pixels);
        let (w, h) = (
            tuiba_core::SCREEN_WIDTH * placement.factor,
            tuiba_core::SCREEN_HEIGHT * placement.factor,
        );
        // At native size the terminal must not stretch the image to a
        // whole number of cells; only a shrink needs the cell box.
        let fit_box = if placement.shrink {
            format!(",c={},r={}", placement.cols, placement.rows)
        } else {
            String::new()
        };
        // `C=1` keeps the cursor where it is.
        let control = format!("a=T,f=24,s={w},v={h}{fit_box},i={IMAGE_ID},p={IMAGE_ID},C=1,q=2");
        if self.transport == Transport::SharedFile {
            if let Ok(file) = self.write_frame_file() {
                write!(out, "\x1b_G{control},t=t;")?;
                base64_into(file.as_os_str().as_encoded_bytes(), out);
                out.extend_from_slice(b"\x1b\\");
                return Ok(());
            }
            // tmpfs went away: fall back for good.
            self.transport = Transport::Direct;
        }
        let mut encoded = Vec::with_capacity(self.pixels.len() / 3 * 4 + 4);
        base64_into(&self.pixels, &mut encoded);
        let chunks = encoded.chunks(CHUNK).collect::<Vec<_>>();
        for (n, chunk) in chunks.iter().enumerate() {
            let more = u8::from(n + 1 < chunks.len());
            if n == 0 {
                write!(out, "\x1b_G{control},t=d,m={more};")?;
            } else {
                write!(out, "\x1b_Gm={more},q=2;")?;
            }
            out.extend_from_slice(chunk);
            out.extend_from_slice(b"\x1b\\");
        }
        Ok(())
    }
}

impl Drop for Kitty {
    fn drop(&mut self) {
        for file in self.recent.drain(..) {
            let _ = fs::remove_file(file);
        }
    }
}
