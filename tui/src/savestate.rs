//! Save states on disk.
//!
//! A state file lives in the state directory rather than next to the
//! ROM: the ROM folder may be read-only, shared, or a mount the user
//! would rather not have written to, and a state is our data, not the
//! cartridge's. Files are named after the ROM, its fingerprint and the
//! slot — `anguna-3f7c1a9e08d4b210-1.state` — so two cartridges with the
//! same file name in different folders do not collide, and a state whose
//! ROM was renamed is still refused by the core rather than silently
//! restored into the wrong game.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tuiba_core::{Cartridge, Snapshot};

use crate::library::state_dir;

/// Save-state slots a cartridge has.
pub const SLOTS: usize = 4;

/// Where states are kept.
#[must_use]
pub fn dir() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("states"))
}

/// One slot's file for one cartridge. Slots are numbered from 1, as the
/// panel shows them.
#[must_use]
pub fn slot_path(rom: &Path, cartridge: &Cartridge, slot: usize) -> Option<PathBuf> {
    let stem = rom.file_stem()?.to_string_lossy().into_owned();
    let stem: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let name = format!("{stem}-{:016x}-{slot}.state", cartridge.fingerprint());
    Some(dir()?.join(name))
}

/// When the state in `path` was written, if it is there at all.
#[must_use]
pub fn written_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Throws a slot away. A slot that is already empty is not an error.
///
/// # Errors
///
/// The file is there but could not be removed.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Writes a state, through a temporary file so that an interrupted write
/// cannot leave a half-written state where a whole one was.
///
/// # Errors
///
/// Any I/O failure along the way, including not having a state directory
/// to write into.
pub fn write(path: &Path, snapshot: &Snapshot) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("state.tmp");
    match fs::write(&tmp, snapshot.to_bytes()).and_then(|()| fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// Reads a state back for `cartridge`, or `None` if there is no file.
///
/// # Errors
///
/// The file exists but could not be read, or is not a state this build
/// can restore into this cartridge.
pub fn read(path: &Path, cartridge: &Cartridge) -> Result<Option<Snapshot>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    Snapshot::from_bytes(&bytes, cartridge)
        .map(Some)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuiba_core::memory::cartridge::HEADER_END;

    fn cartridge(title: &[u8; 12]) -> Cartridge {
        let mut rom = vec![0u8; HEADER_END];
        rom[0..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
        rom[0xA0..0xAC].copy_from_slice(title);
        Cartridge::from_bytes(rom).unwrap()
    }

    #[test]
    fn the_file_name_follows_the_rom_and_its_fingerprint() {
        let cart = cartridge(b"ANGUNA      ");
        let path = slot_path(Path::new("/roms/My Game (demo).gba"), &cart, 2).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            name,
            format!("My-Game--demo--{:016x}-2.state", cart.fingerprint())
        );
        assert!(path.parent().unwrap().ends_with("tuiba/states"));
    }

    /// Two cartridges called the same thing in different folders must not
    /// share a slot.
    #[test]
    fn different_cartridges_get_different_files() {
        let one = slot_path(Path::new("/a/game.gba"), &cartridge(b"ONE         "), 1).unwrap();
        let two = slot_path(Path::new("/b/game.gba"), &cartridge(b"TWO         "), 1).unwrap();
        assert_ne!(one, two);
    }

    #[test]
    fn slots_of_one_cartridge_get_different_files() {
        let cart = cartridge(b"ANGUNA      ");
        let rom = Path::new("/roms/anguna.gba");
        let paths: Vec<_> = (1..=SLOTS)
            .map(|n| slot_path(rom, &cart, n).unwrap())
            .collect();
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), SLOTS);
    }

    #[test]
    fn removing_an_empty_slot_is_not_an_error() {
        let missing = std::env::temp_dir().join("tuiba-no-such-slot-12345.state");
        assert!(remove(&missing).is_ok());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let cart = cartridge(b"ANGUNA      ");
        let missing = std::env::temp_dir().join("tuiba-no-such-state-12345.state");
        let _ = fs::remove_file(&missing);
        assert!(read(&missing, &cart).unwrap().is_none());
    }
}
