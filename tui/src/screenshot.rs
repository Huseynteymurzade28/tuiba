//! Screenshots: the frame on screen, as a PNG in the pictures folder.
//!
//! Shots are the GBA's own 240×160, not the terminal's upscaled view,
//! so they look the same whichever renderer drew them. They go into a
//! `tuiba` folder under the user's pictures directory, named after the
//! ROM and numbered — `anguna-001.png`, `anguna-002.png` — so taking one
//! never overwrites another and needs no clock or time zone.

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::png;

/// Highest number a shot of one ROM gets; past it the folder wants
/// tidying more than it wants a thousandth shot.
const MAX_SHOTS: u32 = 999;

/// Where screenshots go: `tuiba` inside the pictures directory.
#[must_use]
pub fn dir() -> Option<PathBuf> {
    Some(pictures_dir()?.join("tuiba"))
}

/// The user's pictures directory: `XDG_PICTURES_DIR` from the
/// environment or from `user-dirs.dirs` (it is often localised, so
/// guessing `~/Pictures` would miss it), otherwise `~/Pictures`, which
/// is also the name on macOS and Windows.
fn pictures_dir() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    if let Some(dir) = std::env::var_os("XDG_PICTURES_DIR") {
        return Some(PathBuf::from(dir));
    }
    let user_dirs = crate::library::config_dir()
        .and_then(|tuiba| tuiba.parent().map(|config| config.join("user-dirs.dirs")))
        .and_then(|file| fs::read_to_string(file).ok());
    if let Some(dir) = user_dirs.as_deref().and_then(|t| parse_user_dirs(t, &home)) {
        return Some(dir);
    }
    Some(home.join("Pictures"))
}

/// Finds `XDG_PICTURES_DIR` in the text of a `user-dirs.dirs` file.
/// The format is shell-like but fixed: `KEY="$HOME/path"` or an
/// absolute path, one per line.
fn parse_user_dirs(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("XDG_PICTURES_DIR=")
            .map(|v| v.trim_matches('"'))
    })?;
    if let Some(rest) = value.strip_prefix("$HOME") {
        let rest = rest.trim_start_matches('/');
        // `$HOME/` alone means the user switched the folder off.
        return (!rest.is_empty()).then(|| home.join(rest));
    }
    value.starts_with('/').then(|| PathBuf::from(value))
}

/// Saves `frame` as the next free `<rom stem>-NNN.png` in `dir`, and
/// returns where it went.
///
/// # Errors
///
/// The folder cannot be created or written to, or already holds
/// [`MAX_SHOTS`] shots of this ROM.
pub fn save(dir: &Path, rom: &Path, frame: &Framebuffer) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stem = rom
        .file_stem()
        .map_or_else(|| "tuiba".into(), |s| s.to_string_lossy());
    for n in 1..=MAX_SHOTS {
        let path = dir.join(format!("{stem}-{n:03}.png"));
        // `create_new` rather than an exists check: two shots in the
        // same instant (or two tuibas) cannot take the same number.
        let file = match File::options().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        };
        let written = png::write_rgb(
            BufWriter::new(file),
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
            &rgb(frame),
        );
        return match written {
            Ok(()) => Ok(path),
            Err(err) => {
                let _ = fs::remove_file(&path);
                Err(err)
            }
        };
    }
    Err(io::Error::other(format!(
        "{MAX_SHOTS} screenshots of {stem} already"
    )))
}

/// The frame as 8-bit RGB, row-major.
#[must_use]
pub fn rgb(frame: &Framebuffer) -> Vec<u8> {
    frame
        .pixels()
        .iter()
        .flat_map(|&p| [(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dirs_are_read_like_the_shell_would() {
        let home = Path::new("/home/me");
        let text = "# written by xdg-user-dirs-update\n\
                    XDG_DESKTOP_DIR=\"$HOME/Masaüstü\"\n\
                    XDG_PICTURES_DIR=\"$HOME/Resimler\"\n";
        assert_eq!(
            parse_user_dirs(text, home),
            Some(PathBuf::from("/home/me/Resimler"))
        );
        let absolute = "XDG_PICTURES_DIR=\"/data/pics\"\n";
        assert_eq!(
            parse_user_dirs(absolute, home),
            Some(PathBuf::from("/data/pics"))
        );
        assert_eq!(parse_user_dirs("XDG_PICTURES_DIR=\"$HOME/\"\n", home), None);
        assert_eq!(parse_user_dirs("", home), None);
    }

    #[test]
    fn shots_are_numbered_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("tuiba-shots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let frame = Framebuffer::new();
        let rom = Path::new("/roms/Some Game.gba");
        let first = save(&dir, rom, &frame).expect("first shot");
        let second = save(&dir, rom, &frame).expect("second shot");
        assert_eq!(first, dir.join("Some Game-001.png"));
        assert_eq!(second, dir.join("Some Game-002.png"));
        let bytes = fs::read(&first).expect("readable");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
