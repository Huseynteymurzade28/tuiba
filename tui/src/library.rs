//! The ROM library: a list of folders remembered between runs, the
//! cartridges found in them, and which ones were played recently.
//!
//! The folder list lives in `$XDG_CONFIG_HOME/tuiba/library` (or
//! `~/.config/tuiba/library`), one path per line, so it is trivial to
//! edit by hand. `recent` in the same directory holds the last played
//! cartridges, newest first, in the same format.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tuiba_core::memory::Header;
use tuiba_core::memory::SaveType;
use tuiba_core::memory::cartridge::HEADER_END;

/// `$XDG_CONFIG_HOME/tuiba`, or `~/.config/tuiba`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
    Some(base.join("tuiba"))
}

/// Where the folder list is stored.
#[must_use]
pub fn library_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("library"))
}

/// Expands a leading `~` to the home directory.
#[must_use]
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~')
        && let Some(home) = std::env::home_dir()
    {
        let rest = rest.trim_start_matches('/');
        return if rest.is_empty() {
            home
        } else {
            home.join(rest)
        };
    }
    PathBuf::from(path)
}

/// Replaces a leading home directory with `~`, for display.
#[must_use]
pub fn compact_home(path: &Path) -> String {
    if let Some(home) = std::env::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// The remembered folders.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Library {
    /// Folders to scan, in the order they were added.
    pub folders: Vec<PathBuf>,
}

impl Library {
    /// Reads the folder list; a missing file is an empty library.
    #[must_use]
    pub fn load() -> Self {
        library_file()
            .and_then(|file| fs::read_to_string(file).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        Self {
            folders: text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(expand_home)
                .collect(),
        }
    }

    /// Writes the folder list back.
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = library_file() else {
            return Err(io::Error::other("no config directory (HOME unset)"));
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut text = String::new();
        for folder in &self.folders {
            text.push_str(&folder.display().to_string());
            text.push('\n');
        }
        fs::write(file, text)
    }

    /// Adds a folder unless it is already listed. Returns whether the
    /// list changed.
    pub fn add(&mut self, folder: PathBuf) -> bool {
        let folder = folder.canonicalize().unwrap_or(folder);
        if self.folders.contains(&folder) {
            return false;
        }
        self.folders.push(folder);
        true
    }

    /// Removes the folder at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.folders.len() {
            self.folders.remove(index);
        }
    }

    /// Scans every folder (and subfolders up to [`SCAN_DEPTH`] deep) for
    /// `.gba` files, sorted by title.
    #[must_use]
    pub fn scan(&self) -> Vec<Rom> {
        let mut roms = Vec::new();
        for (folder, dir) in self.folders.iter().enumerate() {
            scan_folder(dir, folder, SCAN_DEPTH, &mut roms);
        }
        roms.sort_by_key(Rom::sort_key);
        roms
    }
}

/// How many levels of subfolders a library folder is searched. Enough
/// for `GBA/Homebrew/Jam 2024/`, shallow enough that pointing tuiba at
/// `~` does not walk the whole disk.
pub const SCAN_DEPTH: usize = 3;

/// Collects every `.gba` file in `dir`, descending `depth` more levels.
/// Hidden directories and unreadable ones are skipped.
fn scan_folder(dir: &Path, folder: usize, depth: usize, out: &mut Vec<Rom>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if depth > 0 && !hidden {
                scan_folder(&path, folder, depth - 1, out);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gba"))
            && let Some(rom) = Rom::inspect(path, folder)
        {
            out.push(rom);
        }
    }
}

/// The cartridges played most recently, newest first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recent {
    paths: Vec<PathBuf>,
}

/// How many entries the recent list keeps.
const RECENT_LIMIT: usize = 50;

impl Recent {
    /// Where the list is stored.
    #[must_use]
    pub fn file() -> Option<PathBuf> {
        config_dir().map(|dir| dir.join("recent"))
    }

    /// Reads the list; a missing file is an empty history.
    #[must_use]
    pub fn load() -> Self {
        Self::file()
            .and_then(|file| fs::read_to_string(file).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        Self {
            paths: text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(PathBuf::from)
                .collect(),
        }
    }

    /// Moves `path` to the front.
    pub fn push(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_LIMIT);
    }

    /// Writes the list back.
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = Self::file() else {
            return Err(io::Error::other("no config directory (HOME unset)"));
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut text = String::new();
        for p in &self.paths {
            text.push_str(&p.display().to_string());
            text.push('\n');
        }
        fs::write(file, text)
    }

    /// How recently `path` was played: 0 for the last game, `None` if
    /// never.
    #[must_use]
    pub fn rank(&self, path: &Path) -> Option<usize> {
        self.paths.iter().position(|p| p == path)
    }

    /// The most recently played path, if any.
    #[must_use]
    pub fn last(&self) -> Option<&Path> {
        self.paths.first().map(PathBuf::as_path)
    }
}

/// A cartridge file and what its header says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rom {
    /// The `.gba` file.
    pub path: PathBuf,
    /// Index into [`Library::folders`] of the folder it was found in.
    pub folder: usize,
    /// Parsed header, if the file is large enough to have one.
    pub header: Option<Header>,
    /// File size in bytes.
    pub size: u64,
    /// Whether a `.sav` file sits next to it.
    pub has_save: bool,
    /// Backup chip type; needs the whole file, so read on demand.
    pub save_type: Option<SaveType>,
}

impl Rom {
    fn inspect(path: PathBuf, folder: usize) -> Option<Self> {
        let size = fs::metadata(&path).ok()?.len();
        let mut prefix = vec![0; HEADER_END];
        let header = {
            use std::io::Read;
            let mut file = fs::File::open(&path).ok()?;
            file.read_exact(&mut prefix)
                .ok()
                .and_then(|()| Header::from_prefix(&prefix))
        };
        let has_save = path.with_extension("sav").is_file();
        Some(Self {
            path,
            folder,
            header,
            size,
            has_save,
            save_type: None,
        })
    }

    /// Display name: the header title, or the file name for headerless
    /// or untitled ROMs.
    #[must_use]
    pub fn name(&self) -> String {
        match &self.header {
            Some(h)
                if !matches!(
                    h.title.trim(),
                    "" | "ROM TITLE" | "GAME TITLE" | "GBA" | "AGB"
                ) =>
            {
                h.title.clone()
            }
            _ => self.file_name(),
        }
    }

    /// The game code, unless the header leaves it blank or uses 0000.
    #[must_use]
    pub fn game_code(&self) -> Option<&str> {
        self.header
            .as_ref()
            .map(|header| header.game_code.trim())
            .filter(|code| !code.is_empty() && *code != "0000")
    }

    /// The file name without its extension.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn sort_key(&self) -> (String, PathBuf) {
        (self.name().to_lowercase(), self.path.clone())
    }

    /// Reads the whole file to determine the save type, caching it.
    pub fn save_type(&mut self) -> Option<SaveType> {
        if self.save_type.is_none() {
            let rom = fs::read(&self.path).ok()?;
            self.save_type = Some(SaveType::detect(&rom));
        }
        self.save_type
    }
}

/// `size` in bytes as a short human-readable string.
#[must_use]
pub fn human_size(size: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if size >= MIB {
        // Tenths of a MiB, rounded.
        let tenths = (size * 10).div_ceil(MIB);
        if tenths.is_multiple_of(10) {
            format!("{} MiB", tenths / 10)
        } else {
            format!("{}.{} MiB", tenths / 10, tenths % 10)
        }
    } else {
        format!("{} KiB", size.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folder_list_ignoring_comments_and_blanks() {
        let lib = Library::parse("# roms\n\n/a/b \n  /c\n");
        assert_eq!(
            lib.folders,
            vec![PathBuf::from("/a/b"), PathBuf::from("/c")]
        );
    }

    #[test]
    fn add_is_idempotent_and_remove_is_bounds_checked() {
        let mut lib = Library::default();
        assert!(lib.add(PathBuf::from("/nonexistent/x")));
        assert!(!lib.add(PathBuf::from("/nonexistent/x")));
        lib.remove(5);
        assert_eq!(lib.folders.len(), 1);
        lib.remove(0);
        assert!(lib.folders.is_empty());
    }

    #[test]
    fn scans_a_folder_and_reads_headers() {
        let dir = std::env::temp_dir().join(format!("tuiba-lib-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut rom = vec![0u8; 0x400];
        rom[0xA0..0xA5].copy_from_slice(b"ZELDA");
        rom[0xAC..0xB0].copy_from_slice(b"AZLE");
        rom[0xB0..0xB2].copy_from_slice(b"01");
        rom[0x200..0x208].copy_from_slice(b"EEPROM_V");
        fs::write(dir.join("b.gba"), &rom).unwrap();
        fs::write(dir.join("b.sav"), [0; 8]).unwrap();
        fs::write(dir.join("a.GBA"), [0; 0x10]).unwrap(); // too short for a header
        fs::write(dir.join("notes.txt"), "x").unwrap();
        // Nested folders are searched, hidden ones and too-deep ones not.
        fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        fs::write(dir.join("sub/deeper/c.gba"), [0; 0x10]).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        fs::write(dir.join(".hidden/d.gba"), [0; 0x10]).unwrap();
        fs::create_dir_all(dir.join("1/2/3/4")).unwrap();
        fs::write(dir.join("1/2/3/4/e.gba"), [0; 0x10]).unwrap();

        let lib = Library {
            folders: vec![dir.clone()],
        };
        let mut roms = lib.scan();
        assert_eq!(
            roms.iter().map(Rom::name).collect::<Vec<_>>(),
            ["a", "c", "ZELDA"],
            "a and b at the top, c two levels down; hidden and 4-deep skipped"
        );
        roms.remove(1);
        assert_eq!(
            roms[0].name(),
            "a",
            "headerless ROMs fall back to the file name"
        );
        assert_eq!(roms[0].header, None);
        assert_eq!(roms[1].name(), "ZELDA");
        assert!(roms[1].has_save);
        assert_eq!(roms[1].size, 0x400);
        assert_eq!(roms[1].save_type(), Some(SaveType::Eeprom));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn placeholder_header_fields_are_treated_as_absent() {
        let rom = Rom {
            path: PathBuf::from("homebrew.gba"),
            folder: 0,
            header: Some(Header {
                title: "ROM TITLE".to_string(),
                game_code: "0000".to_string(),
                maker_code: String::new(),
                version: 0,
                checksum: 0,
                valid: false,
            }),
            size: 0,
            has_save: false,
            save_type: None,
        };

        assert_eq!(rom.name(), "homebrew");
        assert_eq!(rom.game_code(), None);
    }

    #[test]
    fn sizes_read_nicely() {
        assert_eq!(human_size(0x400), "1 KiB");
        assert_eq!(human_size(16 * 1024 * 1024), "16 MiB");
        assert_eq!(human_size(1024 * 1024 + 512 * 1024), "1.5 MiB");
    }

    #[test]
    fn home_expansion_round_trips() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_home("~/x"), home.join("x"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
        assert_eq!(compact_home(&home.join("y")), "~/y");
        assert_eq!(compact_home(Path::new("/etc")), "/etc");
    }

    #[test]
    fn recent_ranks_newest_first_and_dedups() {
        let mut recent = Recent::parse("/x/a.gba\n/x/b.gba\n");
        assert_eq!(recent.rank(Path::new("/x/b.gba")), Some(1));
        assert_eq!(recent.rank(Path::new("/x/z.gba")), None);
        assert_eq!(recent.last(), Some(Path::new("/x/a.gba")));
        recent.push(Path::new("/x/b.gba"));
        assert_eq!(recent.rank(Path::new("/x/b.gba")), Some(0));
        assert_eq!(recent.rank(Path::new("/x/a.gba")), Some(1));
        assert_eq!(recent.paths.len(), 2, "moved, not duplicated");
        for i in 0..100 {
            recent.push(Path::new(&format!("/x/{i}.gba")));
        }
        assert_eq!(recent.paths.len(), RECENT_LIMIT);
    }
}
