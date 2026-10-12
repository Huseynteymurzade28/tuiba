//! Finding the player's BIOS image.
//!
//! The GBA BIOS is Nintendo's and cannot ship with tuiba; players who
//! dumped their own console's can point `--bios` at it, or drop it in
//! the config directory as `gba_bios.bin` to have every game use it.
//! Without one, the core's built-in replacement (HLE) runs instead.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use tuiba_core::memory::BIOS_SIZE;

use crate::{library, zip};

/// The name the config directory's image goes by.
pub const FILE_NAME: &str = "gba_bios.bin";

/// CRC-32 of the BIOS in a Game Boy Advance.
const GBA_CRC: u32 = 0x8197_7335;
/// CRC-32 of the GBA BIOS as dumped from a Nintendo DS, which differs
/// from the console's in a few bytes and works the same.
const NDS_CRC: u32 = 0xA647_3709;

/// Which BIOS an image is, as far as we can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Game Boy Advance's own BIOS.
    Gba,
    /// The GBA BIOS from a Nintendo DS.
    Nds,
    /// Something else of the right size: a replacement BIOS, a
    /// modified one, or a bad dump.
    Unknown,
}

impl Kind {
    fn of(image: &[u8]) -> Self {
        match zip::crc32(image) {
            GBA_CRC => Self::Gba,
            NDS_CRC => Self::Nds,
            _ => Self::Unknown,
        }
    }
}

/// A BIOS image read from disk.
#[derive(Debug, Clone)]
pub struct Bios {
    /// The image itself, exactly [`BIOS_SIZE`] bytes.
    pub image: Vec<u8>,
    /// Which BIOS it is.
    pub kind: Kind,
}

/// Why a BIOS image could not be used.
#[derive(Debug)]
pub struct BiosError {
    /// The file that was tried.
    pub path: PathBuf,
    /// What went wrong.
    pub reason: Reason,
}

/// What went wrong with a BIOS file.
#[derive(Debug)]
pub enum Reason {
    /// It could not be read.
    Io(io::Error),
    /// It is not 16 KiB.
    Size(usize),
}

impl fmt::Display for BiosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = library::compact_home(&self.path);
        match &self.reason {
            Reason::Io(err) => write!(f, "BIOS {path}: {err}"),
            Reason::Size(size) => write!(
                f,
                "BIOS {path} is {size} bytes; a GBA BIOS is {BIOS_SIZE} (16 KiB)"
            ),
        }
    }
}

impl std::error::Error for BiosError {}

/// Where an image dropped in the config directory lives.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    library::config_dir().map(|dir| dir.join(FILE_NAME))
}

/// The BIOS to run with: the one named on the command line, else the one
/// in the config directory if there is one, else none.
///
/// # Errors
///
/// An image that is named or present but unreadable or the wrong size is
/// an error rather than a silent fallback to HLE: whoever put it there
/// wants it used.
pub fn find(arg: Option<&Path>) -> Result<Option<Bios>, BiosError> {
    match arg {
        Some(path) => read(path).map(Some),
        None => match default_path() {
            Some(path) if path.is_file() => read(&path).map(Some),
            _ => Ok(None),
        },
    }
}

/// Reads and checks the image at `path`.
fn read(path: &Path) -> Result<Bios, BiosError> {
    let error = |reason| BiosError {
        path: path.to_path_buf(),
        reason,
    };
    let image = std::fs::read(path).map_err(|e| error(Reason::Io(e)))?;
    if image.len() != BIOS_SIZE {
        return Err(error(Reason::Size(image.len())));
    }
    let kind = Kind::of(&image);
    Ok(Bios { image, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tuiba-bios-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reads_an_image_of_the_right_size() {
        let path = temp_file("ok", &[0; BIOS_SIZE]);
        let bios = find(Some(&path)).unwrap().unwrap();
        assert_eq!(bios.image.len(), BIOS_SIZE);
        assert_eq!(bios.kind, Kind::Unknown);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_a_wrong_size_or_missing_file() {
        let path = temp_file("short", &[0; 100]);
        let err = find(Some(&path)).unwrap_err();
        assert!(matches!(err.reason, Reason::Size(100)));
        assert!(err.to_string().contains("16 KiB"));
        std::fs::remove_file(&path).unwrap();
        let err = find(Some(&path)).unwrap_err();
        assert!(matches!(err.reason, Reason::Io(_)));
    }
}
