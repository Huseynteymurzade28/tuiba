//! Just enough of the ZIP format to play cartridges straight from their
//! archives.
//!
//! An archive is read through its central directory, the table of
//! contents at the end of the file, so listing one costs a seek and a
//! small read however large it is. Members may be stored or deflated
//! (what every zip tool writes); deflate goes through `miniz_oxide`.
//! ZIP64, encryption and the other methods are refused with an error
//! rather than misread — a cartridge is at most 32 MiB, so ZIP64 never
//! matters for one.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// End of central directory record signature, `PK\x05\x06`.
const EOCD_SIGNATURE: u32 = 0x0605_4B50;
/// Central directory file header signature, `PK\x01\x02`.
const CENTRAL_SIGNATURE: u32 = 0x0201_4B50;
/// Local file header signature, `PK\x03\x04`.
const LOCAL_SIGNATURE: u32 = 0x0403_4B50;
/// Fixed part of the end of central directory record.
const EOCD_LEN: usize = 22;
/// Fixed part of a central directory file header.
const CENTRAL_LEN: usize = 46;
/// Fixed part of a local file header.
const LOCAL_LEN: usize = 30;
/// The record ends with a comment of up to this many bytes.
const MAX_COMMENT: usize = 0xFFFF;
/// General purpose flag: the member is encrypted.
const FLAG_ENCRYPTED: u16 = 1;

/// How a member's bytes are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Stored,
    Deflated,
}

/// One file in an archive, as the central directory describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Path inside the archive, `/`-separated.
    pub name: String,
    /// Size once extracted.
    pub size: u64,
    compressed_size: u64,
    crc: u32,
    method: Option<Method>,
    encrypted: bool,
    /// Offset of the member's local header.
    offset: u64,
}

impl Member {
    /// Whether the member is a directory rather than a file.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    /// The last component of [`Member::name`].
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }
}

/// An open archive and its table of contents.
#[derive(Debug)]
pub struct Archive {
    file: File,
    path: PathBuf,
    members: Vec<Member>,
}

impl Archive {
    /// Opens `path` and reads its central directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let members = read_directory(&mut file)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            members,
        })
    }

    /// Every member, in directory order.
    #[must_use]
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The member called `name`.
    #[must_use]
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Extracts `member` whole, checking its size and CRC-32.
    pub fn read(&mut self, member: &Member) -> io::Result<Vec<u8>> {
        let data = self.raw(member, member.compressed_size)?;
        let out = match self.method(member)? {
            Method::Stored => data,
            Method::Deflated => {
                let size = usize::try_from(member.size).map_err(|_| self.corrupt(member))?;
                miniz_oxide::inflate::decompress_to_vec_with_limit(&data, size)
                    .map_err(|_| self.corrupt(member))?
            }
        };
        if out.len() as u64 != member.size || crc32(&out) != member.crc {
            return Err(self.corrupt(member));
        }
        Ok(out)
    }

    /// Extracts up to the first `len` bytes of `member`, without checks.
    /// Cheap however large the member is.
    pub fn read_prefix(&mut self, member: &Member, len: usize) -> io::Result<Vec<u8>> {
        let len = len.min(usize::try_from(member.size).unwrap_or(usize::MAX));
        match self.method(member)? {
            Method::Stored => self.raw(member, len as u64),
            Method::Deflated => {
                // A deflate stream never needs more than this much input
                // per byte of output plus its block headers, so the read
                // stays small for a large member.
                let budget = (len as u64 + 1024) * 2;
                let data = self.raw(member, member.compressed_size.min(budget))?;
                let mut out = vec![0; len];
                let mut state = DecompressorOxide::new();
                let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
                    | inflate_flags::TINFL_FLAG_HAS_MORE_INPUT;
                let (status, _, produced) = decompress(&mut state, &data, &mut out, 0, flags);
                if !matches!(
                    status,
                    TINFLStatus::Done | TINFLStatus::HasMoreOutput | TINFLStatus::NeedsMoreInput
                ) {
                    return Err(self.corrupt(member));
                }
                out.truncate(produced);
                Ok(out)
            }
        }
    }

    fn method(&self, member: &Member) -> io::Result<Method> {
        if member.encrypted {
            return Err(self.unsupported(member, "is encrypted"));
        }
        member
            .method
            .ok_or_else(|| self.unsupported(member, "uses an unsupported compression method"))
    }

    /// The first `len` stored bytes of `member`, past its local header.
    fn raw(&mut self, member: &Member, len: u64) -> io::Result<Vec<u8>> {
        let mut local = [0; LOCAL_LEN];
        self.file.seek(SeekFrom::Start(member.offset))?;
        self.file.read_exact(&mut local)?;
        if le32(&local, 0) != LOCAL_SIGNATURE {
            return Err(self.corrupt(member));
        }
        // The local header repeats the name and has its own extra field,
        // not necessarily the same length as the central one.
        let skip = u64::from(le16(&local, 26)) + u64::from(le16(&local, 28));
        self.file.seek(SeekFrom::Current(skip as i64))?;
        let mut data = Vec::new();
        (&mut self.file).take(len).read_to_end(&mut data)?;
        if data.len() as u64 != len {
            return Err(self.corrupt(member));
        }
        Ok(data)
    }

    fn corrupt(&self, member: &Member) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {} is damaged", self.path.display(), member.name),
        )
    }

    fn unsupported(&self, member: &Member, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("{}: {} {why}", self.path.display(), member.name),
        )
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("not a zip archive: {what}"),
    )
}

/// Finds the end of central directory record and parses the directory
/// it points to.
fn read_directory(file: &mut File) -> io::Result<Vec<Member>> {
    let len = file.seek(SeekFrom::End(0))?;
    let tail_len = len.min((EOCD_LEN + MAX_COMMENT) as u64);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    if tail.len() < EOCD_LEN {
        return Err(invalid("too short"));
    }
    // Search backwards: the comment may itself contain the signature,
    // the real record is the last one whose comment reaches the end.
    let eocd = (0..=tail.len() - EOCD_LEN)
        .rev()
        .find(|&at| {
            le32(&tail, at) == EOCD_SIGNATURE
                && at + EOCD_LEN + usize::from(le16(&tail, at + 20)) == tail.len()
        })
        .ok_or_else(|| invalid("no end of central directory"))?;
    let count = usize::from(le16(&tail, eocd + 10));
    let size = le32(&tail, eocd + 12);
    let offset = le32(&tail, eocd + 16);
    if size == u32::MAX || offset == u32::MAX || count == 0xFFFF {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "ZIP64 archives are not supported",
        ));
    }
    if u64::from(offset) + u64::from(size) > len {
        return Err(invalid("central directory out of bounds"));
    }
    file.seek(SeekFrom::Start(offset.into()))?;
    let mut dir = vec![0; size as usize];
    file.read_exact(&mut dir)?;
    parse_directory(&dir, count)
}

fn parse_directory(dir: &[u8], count: usize) -> io::Result<Vec<Member>> {
    let mut members = Vec::with_capacity(count);
    let mut at = 0;
    for _ in 0..count {
        let header = dir
            .get(at..at + CENTRAL_LEN)
            .filter(|h| le32(h, 0) == CENTRAL_SIGNATURE)
            .ok_or_else(|| invalid("bad central directory entry"))?;
        let name_len = usize::from(le16(header, 28));
        let extra_len = usize::from(le16(header, 30));
        let comment_len = usize::from(le16(header, 32));
        let name = dir
            .get(at + CENTRAL_LEN..at + CENTRAL_LEN + name_len)
            .ok_or_else(|| invalid("truncated central directory"))?;
        members.push(Member {
            // Names are CP437 unless flagged UTF-8; for the ASCII that
            // ROM names are in practice, both read the same.
            name: String::from_utf8_lossy(name).into_owned(),
            size: le32(header, 24).into(),
            compressed_size: le32(header, 20).into(),
            crc: le32(header, 16),
            method: match le16(header, 10) {
                0 => Some(Method::Stored),
                8 => Some(Method::Deflated),
                _ => None,
            },
            encrypted: le16(header, 8) & FLAG_ENCRYPTED != 0,
            offset: le32(header, 42).into(),
        });
        at += CENTRAL_LEN + name_len + extra_len + comment_len;
    }
    Ok(members)
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// CRC-32 (IEEE), as zip stores it.
fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut i = 0;
        while i < 256 {
            let mut crc = i as u32;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 == 0 {
                    crc >> 1
                } else {
                    (crc >> 1) ^ 0xEDB8_8320
                };
                bit += 1;
            }
            table[i] = crc;
            i += 1;
        }
        table
    };
    !data.iter().fold(0xFFFF_FFFF, |crc, &b| {
        TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds an archive of `(name, data, deflate)` members, the way a
    /// zip tool lays one out.
    pub(crate) fn build(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for &(name, data, deflate) in members {
            let (method, stored) = if deflate {
                (8u16, miniz_oxide::deflate::compress_to_vec(data, 6))
            } else {
                (0, data.to_vec())
            };
            let offset = out.len() as u32;
            let common = |buf: &mut Vec<u8>| {
                buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
                buf.extend_from_slice(&0u16.to_le_bytes()); // flags
                buf.extend_from_slice(&method.to_le_bytes());
                buf.extend_from_slice(&[0; 4]); // time, date
                buf.extend_from_slice(&crc32(data).to_le_bytes());
                buf.extend_from_slice(&(stored.len() as u32).to_le_bytes());
                buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
                buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
            };
            out.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
            common(&mut out);
            out.extend_from_slice(&3u16.to_le_bytes()); // extra field
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[0; 3]);
            out.extend_from_slice(&stored);

            central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            common(&mut central);
            central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let dir_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&[0; 4]); // disk numbers
        let count = (members.len() as u16).to_le_bytes();
        out.extend_from_slice(&count);
        out.extend_from_slice(&count);
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        let comment = b"PK\x05\x06 lookalike";
        out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        out.extend_from_slice(comment);
        out
    }

    fn sample() -> Vec<u8> {
        (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn open(bytes: &[u8], name: &str) -> Archive {
        let path = std::env::temp_dir().join(format!("tuiba-zip-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let zip = Archive::open(&path).unwrap();
        // The open handle keeps the data readable on Unix; Windows
        // refuses the removal, which only leaves a file in the temp dir.
        let _ = std::fs::remove_file(&path);
        zip
    }

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn lists_and_extracts_stored_and_deflated_members() {
        let data = sample();
        let mut zip = open(
            &build(&[
                ("docs/", b"", false),
                ("docs/readme.txt", b"hello", false),
                ("game.gba", &data, true),
            ]),
            "members",
        );
        let names: Vec<_> = zip.members().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["docs/", "docs/readme.txt", "game.gba"]);
        assert!(zip.members()[0].is_dir());
        assert_eq!(zip.members()[1].file_name(), "readme.txt");

        let readme = zip.member("docs/readme.txt").unwrap().clone();
        assert_eq!(zip.read(&readme).unwrap(), b"hello");
        let game = zip.member("game.gba").unwrap().clone();
        assert_eq!(game.size, data.len() as u64);
        assert_eq!(zip.read(&game).unwrap(), data);
        assert_eq!(zip.read_prefix(&game, 0xC0).unwrap(), data[..0xC0]);
        assert_eq!(zip.read_prefix(&readme, 0xC0).unwrap(), b"hello");
    }

    #[test]
    fn refuses_damaged_members() {
        let data = sample();
        let mut bytes = build(&[("game.gba", &data, false)]);
        bytes[LOCAL_LEN + "game.gba".len() + 3 + 500] ^= 1;
        let mut zip = open(&bytes, "damaged");
        let game = zip.members()[0].clone();
        let err = zip.read(&game).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn refuses_files_that_are_not_archives() {
        let path = std::env::temp_dir().join(format!("tuiba-zip-{}-fake", std::process::id()));
        std::fs::write(&path, vec![0x55; 4000]).unwrap();
        let err = Archive::open(&path).unwrap_err();
        std::fs::write(&path, b"").unwrap();
        let empty = Archive::open(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(empty.is_err());
    }
}
