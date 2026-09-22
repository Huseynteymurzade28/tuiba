//! The frame the library shows for a cartridge.
//!
//! A preview is written by the game itself: whenever a save state is
//! taken, and once more when the player leaves, the frame on screen is
//! dropped into the cache directory. The library then has something
//! true to show — where you actually were — without emulating anything
//! or even opening the ROM.
//!
//! The file is a raw frame rather than a PNG: it is ours to read back,
//! and reading a PNG would mean carrying a decoder for pictures nobody
//! but us writes. It is a cache in every sense; deleting it costs a
//! screenshot, nothing more.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::library::cache_dir;

/// Marks the file as ours and says what is in it.
const MAGIC: [u8; 8] = *b"TUIBAPRV";
/// Bumped if the pixel format below ever changes.
const FORMAT_VERSION: u16 = 1;
/// Magic, version, and then one `u32` per pixel.
const HEADER_LEN: usize = MAGIC.len() + 2;
const PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;
const FILE_LEN: usize = HEADER_LEN + PIXELS * 4;

/// Where previews are kept.
#[must_use]
pub fn dir() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("previews"))
}

/// The preview file for a ROM.
///
/// Named after the cartridge's path, not its contents: the library knows
/// the path of every ROM it lists, and hashing the file to learn more
/// would mean reading megabytes for a thumbnail.
#[must_use]
pub fn path(rom: &Path) -> Option<PathBuf> {
    let stem: String = rom
        .file_stem()?
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    Some(dir()?.join(format!("{stem}-{:08x}.frame", hash_path(rom))))
}

/// FNV-1a over the path, to keep two cartridges of the same name apart.
fn hash_path(rom: &Path) -> u32 {
    const OFFSET: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    rom.to_string_lossy().bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(PRIME)
    })
}

/// Writes a frame, replacing whatever was there.
///
/// Best effort by nature: the caller is on its way out of a game, and a
/// thumbnail is never worth an error in the player's face.
pub fn write(rom: &Path, frame: &Framebuffer) {
    let Some(path) = path(rom) else { return };
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = Vec::with_capacity(FILE_LEN);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    for pixel in frame.pixels() {
        bytes.extend_from_slice(&pixel.to_le_bytes());
    }
    let tmp = path.with_extension("frame.tmp");
    if fs::write(&tmp, &bytes)
        .and_then(|()| fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = fs::remove_file(&tmp);
    }
}

/// Reads a ROM's preview, and when it was taken.
///
/// Anything unreadable, truncated or written by another format version
/// is simply no preview: the library draws an empty frame instead of an
/// error nobody can act on.
#[must_use]
pub fn read(rom: &Path) -> Option<(Framebuffer, SystemTime)> {
    let path = path(rom)?;
    let taken_at = fs::metadata(&path).ok()?.modified().ok()?;
    let bytes = fs::read(&path).ok()?;
    if bytes.len() != FILE_LEN || bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    if u16::from_le_bytes([bytes[8], bytes[9]]) != FORMAT_VERSION {
        return None;
    }
    let mut frame = Framebuffer::new();
    for y in 0..SCREEN_HEIGHT {
        for (x, pixel) in frame.row_mut(y).iter_mut().enumerate() {
            let at = HEADER_LEN + (y * SCREEN_WIDTH + x) * 4;
            *pixel = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        }
    }
    Some((frame, taken_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_roms_of_the_same_name_get_different_files() {
        let one = path(Path::new("/games/a/zelda.gba")).unwrap();
        let two = path(Path::new("/games/b/zelda.gba")).unwrap();
        assert_ne!(one, two);
        assert!(
            one.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("zelda-")
        );
    }

    #[test]
    fn a_frame_survives_the_round_trip() {
        let dir = std::env::temp_dir().join("tuiba-preview-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("round-trip.gba");

        let mut frame = Framebuffer::new();
        for y in 0..SCREEN_HEIGHT {
            for (x, pixel) in frame.row_mut(y).iter_mut().enumerate() {
                *pixel = ((x * 7 + y * 13) as u32) << 8 | 0xFF;
            }
        }
        // The cache directory is wherever the environment says; write and
        // read through the real path so the naming is exercised too.
        write(&rom, &frame);
        let (read_back, _) = read(&rom).expect("the preview just written");
        assert_eq!(read_back.pixels(), frame.pixels());
        let _ = fs::remove_file(path(&rom).unwrap());
    }

    #[test]
    fn nonsense_in_the_cache_is_not_a_preview() {
        let rom = std::env::temp_dir().join("tuiba-preview-garbage.gba");
        let path = path(&rom).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not a frame").unwrap();
        assert!(read(&rom).is_none());
        let _ = fs::remove_file(&path);
    }
}
