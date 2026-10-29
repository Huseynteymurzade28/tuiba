//! The last frame each cartridge showed, kept so the library can preview
//! a game before it is started.
//!
//! Covers live in the state directory next to save states, one per ROM,
//! as raw 240×160 RGB behind a short magic. They are a convenience: a
//! missing, short or unreadable file just means no preview.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::library::state_dir;

const MAGIC: &[u8; 8] = b"TBCOVER1";
const PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

/// Where covers are kept.
#[must_use]
pub fn dir() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("covers"))
}

/// The cover file for the cartridge at `rom` (its shown path, so two
/// members of one archive get their own).
#[must_use]
pub fn path_for(rom: &Path) -> Option<PathBuf> {
    Some(dir()?.join(file_name(rom)))
}

/// `stem-hash.cover`: readable, and unique per full path.
fn file_name(rom: &Path) -> String {
    let stem: String = rom
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    format!(
        "{stem}-{:016x}.cover",
        fnv1a(rom.to_string_lossy().as_bytes())
    )
}

/// 64-bit FNV-1a: stable across builds, unlike the standard hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Stores `frame` as the cover of `rom`.
///
/// # Errors
///
/// The state directory is unknown or the file could not be written.
pub fn save(rom: &Path, frame: &Framebuffer) -> io::Result<()> {
    let path = path_for(rom).ok_or_else(|| io::Error::other("no state directory"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("cover.tmp");
    fs::write(&tmp, encode(frame))?;
    fs::rename(tmp, path)
}

/// The cover of `rom`, if one was saved and is intact.
#[must_use]
pub fn load(rom: &Path) -> Option<Framebuffer> {
    decode(&fs::read(path_for(rom)?).ok()?)
}

fn encode(frame: &Framebuffer) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MAGIC.len() + PIXELS * 3);
    bytes.extend_from_slice(MAGIC);
    for &p in frame.pixels() {
        bytes.extend_from_slice(&[(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8]);
    }
    bytes
}

fn decode(bytes: &[u8]) -> Option<Framebuffer> {
    let rgb = bytes.strip_prefix(MAGIC)?;
    if rgb.len() != PIXELS * 3 {
        return None;
    }
    let mut frame = Framebuffer::new();
    for y in 0..SCREEN_HEIGHT {
        let src = &rgb[y * SCREEN_WIDTH * 3..(y + 1) * SCREEN_WIDTH * 3];
        for (dst, &[r, g, b]) in frame.row_mut(y).iter_mut().zip(src.as_chunks::<3>().0) {
            *dst = u32::from_be_bytes([r, g, b, 0xFF]);
        }
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_frame() {
        let mut frame = Framebuffer::new();
        frame.row_mut(7)[9] = 0x1234_56FF;
        let back = decode(&encode(&frame)).unwrap();
        assert_eq!(back.rgb(9, 7), (0x12, 0x34, 0x56));
        assert_eq!(back.rgb(0, 0), (0, 0, 0));
    }

    #[test]
    fn rejects_foreign_or_short_files() {
        assert!(decode(b"nope").is_none());
        let mut bytes = encode(&Framebuffer::new());
        bytes.pop();
        assert!(decode(&bytes).is_none());
    }

    #[test]
    fn names_are_unique_per_path() {
        let a = file_name(Path::new("/roms/a/game.gba"));
        let b = file_name(Path::new("/roms/b/game.gba"));
        assert_ne!(a, b);
        assert!(a.starts_with("game-"));
        assert_eq!(Path::new(&a).extension(), Some("cover".as_ref()));
    }
}
