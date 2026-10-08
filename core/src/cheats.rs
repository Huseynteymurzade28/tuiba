//! Cheat codes: GameShark, Action Replay, CodeBreaker and raw writes.
//!
//! A cheat is one or more lines of a code as printed in a magazine or a
//! code list. [`Cheat::parse`] decodes them — decrypting where the device
//! encrypted its codes — into a small program of writes and conditions,
//! and [`Cheat::apply`] runs that program against the machine. A frontend
//! applies its enabled cheats once per frame, between frames, which is
//! what the cartridge devices did from their hook in the game's
//! interrupt handler.
//!
//! The formats:
//!
//! - **Raw** (`02001234:63`): a write of the value's width — two, four or
//!   eight hex digits — every frame. The format VBA and many code lists use.
//! - **GameShark** v1/v2 (`XXXXXXXX YYYYYYYY`): TEA-encrypted lines.
//! - **Action Replay** v3 (`XXXXXXXX YYYYYYYY`): TEA with other keys and a
//!   richer instruction set; also what the GameShark SP and later
//!   GameShark Advance releases use.
//! - **CodeBreaker** (`XXXXXXXX YYYY`): plain lines.
//!
//! The two encrypted formats look the same on paper. Without an explicit
//! format, [`Cheat::parse`] decrypts the lines both ways and keeps the
//! reading whose addresses land in real memory.
//!
//! Some codes need what an emulator between frames does not have: ROM
//! patches, the device's own button, slowdown. Those lines are skipped and
//! listed in [`Cheat::skipped`]; the rest of the cheat still works.
//! Re-keyed codes (`DEADFACE`, CodeBreaker type 9) change how every
//! later line decrypts and are refused.

use crate::gba::Gba;
use crate::memory::io::reg;
use crate::memory::{Memory, MemoryRegion};

/// The code formats [`Cheat::parse`] understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// `AAAAAAAA:VV`, `:VVVV` or `:VVVVVVVV` — a plain write.
    Raw,
    /// GameShark Advance v1/v2.
    GameShark,
    /// Pro Action Replay v3 (and GameShark SP).
    ActionReplay,
    /// CodeBreaker.
    CodeBreaker,
}

impl Format {
    /// All formats.
    pub const ALL: [Self; 4] = [
        Self::Raw,
        Self::CodeBreaker,
        Self::ActionReplay,
        Self::GameShark,
    ];

    /// The short name used in cheat files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::GameShark => "gs",
            Self::ActionReplay => "ar",
            Self::CodeBreaker => "cb",
        }
    }

    /// Reads a name as written in a cheat file; a few spellings each.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "raw" | "vba" => Self::Raw,
            "gs" | "gsa" | "gameshark" => Self::GameShark,
            "ar" | "par" | "ar3" | "par3" | "actionreplay" => Self::ActionReplay,
            "cb" | "codebreaker" => Self::CodeBreaker,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Raw => "raw",
            Self::GameShark => "GameShark",
            Self::ActionReplay => "Action Replay",
            Self::CodeBreaker => "CodeBreaker",
        })
    }
}

/// Why a cheat could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheatError {
    /// There were no code lines at all.
    #[error("the cheat has no code lines")]
    Empty,
    /// A line is not hex in the shape the format needs.
    #[error("line {line}: `{text}` is not a {format} code")]
    Shape {
        /// One-based line number within the cheat.
        line: usize,
        /// The line as given.
        text: String,
        /// The format it was read as.
        format: Format,
    },
    /// The lines decode but do not mean anything known.
    #[error("line {line}: {reason}")]
    Unsupported {
        /// One-based line number within the cheat.
        line: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// No format reads every line.
    #[error("not a code tuiba recognises (raw, GameShark, Action Replay or CodeBreaker)")]
    Unrecognised,
}

/// Width of one memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Byte,
    Half,
    Word,
}

impl Width {
    /// From a size in bytes: 1, 2 or 4.
    fn bytes(n: u32) -> Option<Self> {
        match n {
            1 => Some(Self::Byte),
            2 => Some(Self::Half),
            4 => Some(Self::Word),
            _ => None,
        }
    }

    const fn size(self) -> u32 {
        match self {
            Self::Byte => 1,
            Self::Half => 2,
            Self::Word => 4,
        }
    }

    const fn mask(self, value: u32) -> u32 {
        match self {
            Self::Byte => value & 0xFF,
            Self::Half => value & 0xFFFF,
            Self::Word => value,
        }
    }

    /// `value` sign-extended from this width.
    #[allow(clippy::cast_possible_wrap)]
    const fn signed(self, value: u32) -> i32 {
        match self {
            Self::Byte => value as u8 as i8 as i32,
            Self::Half => value as u16 as i16 as i32,
            Self::Word => value as i32,
        }
    }
}

/// How a condition compares memory (left) with its value (right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Test {
    Eq,
    Ne,
    /// Signed at the access width.
    Lt,
    Gt,
    /// Unsigned.
    Ult,
    Ugt,
    Ule,
    Uge,
    /// Some bit of the value is set in memory.
    Any,
    /// No bit of the value is set in memory.
    None,
    /// Never true: Action Replay's "false" width.
    Never,
}

impl Test {
    fn holds(self, width: Width, memory: u32, value: u32) -> bool {
        match self {
            Self::Eq => memory == value,
            Self::Ne => memory != value,
            Self::Lt => width.signed(memory) < width.signed(value),
            Self::Gt => width.signed(memory) > width.signed(value),
            Self::Ult => memory < value,
            Self::Ugt => memory > value,
            Self::Ule => memory <= value,
            Self::Uge => memory >= value,
            Self::Any => memory & value != 0,
            Self::None => memory & value == 0,
            Self::Never => false,
        }
    }
}

/// Read-modify-write operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modify {
    Add,
    Or,
    And,
}

/// One instruction of a decoded cheat.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// Writes `count` units, stepping the address and the value after
    /// each — a plain write when `count` is 1.
    Write {
        address: u32,
        width: Width,
        value: u32,
        count: u32,
        address_step: u32,
        value_step: u32,
    },
    /// Writes `value` at the word stored at `pointer`, plus `offset`.
    WriteIndirect {
        pointer: u32,
        offset: u32,
        width: Width,
        value: u32,
    },
    /// Changes the unit at `address` in place.
    Modify {
        address: u32,
        width: Width,
        how: Modify,
        value: u32,
    },
    /// Runs the next `then` ops when the test holds, otherwise skips
    /// them and runs the `otherwise` ops after them.
    If {
        address: u32,
        width: Width,
        test: Test,
        value: u32,
        then: usize,
        otherwise: usize,
    },
}

impl Op {
    fn write(address: u32, width: Width, value: u32) -> Self {
        Self::Write {
            address,
            width,
            value: width.mask(value),
            count: 1,
            address_step: 0,
            value_step: 0,
        }
    }

    fn check(address: u32, width: Width, test: Test, value: u32, then: usize) -> Self {
        Self::If {
            address,
            width,
            test,
            value: width.mask(value),
            then,
            otherwise: 0,
        }
    }
}

/// A decoded cheat, ready to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cheat {
    format: Format,
    ops: Vec<Op>,
    skipped: Vec<String>,
}

impl Cheat {
    /// Decodes the code lines in `text`, one code per line; blank lines
    /// are ignored. With `format` unset the format is detected.
    ///
    /// # Errors
    ///
    /// When a line does not fit the format, decodes to nothing known, or
    /// — undetected — no format reads all the lines.
    pub fn parse(text: &str, format: Option<Format>) -> Result<Self, CheatError> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return Err(CheatError::Empty);
        }
        if let Some(format) = format {
            return decode(&lines, format);
        }
        let all = |fits: fn(&str) -> bool| lines.iter().all(|l| fits(l));
        if all(|l| raw_line(l).is_some()) {
            return decode(&lines, Format::Raw);
        }
        if all(|l| codebreaker_line(l).is_some()) {
            return decode(&lines, Format::CodeBreaker);
        }
        if !all(|l| pair_line(l).is_some()) {
            return Err(CheatError::Unrecognised);
        }
        // The encrypted formats: the right key gives real addresses, the
        // wrong one noise.
        let readings = [Format::ActionReplay, Format::GameShark].map(|f| decode(&lines, f));
        let best = readings
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .max_by_key(|cheat| cheat.plausibility())
            .filter(|cheat| cheat.plausibility() > 0);
        if let Some(cheat) = best {
            return Ok(cheat.clone());
        }
        // A line that decodes but means nothing known says more than
        // "unrecognised".
        readings
            .into_iter()
            .find_map(|r| match r {
                Err(err @ CheatError::Unsupported { .. }) => Some(err),
                _ => None,
            })
            .map_or(Err(CheatError::Unrecognised), Err)
    }

    /// The format the lines were read as.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }

    /// Lines that were understood but cannot work here (ROM patches,
    /// device buttons), as human-readable notes.
    #[must_use]
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Whether anything is left to do once the skipped lines are gone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Runs the cheat once against the machine. Writes to the BIOS and
    /// the cartridge ROM are dropped.
    pub fn apply(&self, gba: &mut Gba) {
        run(&self.ops, gba);
        // The accesses above are the cheat device's, not the CPU's.
        gba.bus.take_access_cycles();
    }

    /// How believable this reading is: ops on real memory score, ops
    /// elsewhere cost. Used to tell the two encrypted formats apart,
    /// where the wrong key turns every line into noise.
    fn plausibility(&self) -> i32 {
        let real = |address: u32| match MemoryRegion::from_address(address) {
            Some(MemoryRegion::Ewram | MemoryRegion::Iwram) => 4,
            Some(
                MemoryRegion::Io
                | MemoryRegion::Palette
                | MemoryRegion::Vram
                | MemoryRegion::Oam
                | MemoryRegion::Sram,
            ) => 1,
            _ => -8,
        };
        let ops: i32 = self
            .ops
            .iter()
            .map(|op| match *op {
                Op::Write { address, .. } | Op::Modify { address, .. } | Op::If { address, .. } => {
                    real(address)
                }
                Op::WriteIndirect { pointer, .. } => real(pointer),
            })
            .sum();
        // A cheat whose lines are all markers (master codes) is still a
        // good reading, just a quiet one.
        ops + i32::from(self.ops.is_empty())
    }
}

/// Runs `ops`, recursing into the branches of conditions.
fn run(ops: &[Op], gba: &mut Gba) {
    let mut i = 0;
    while i < ops.len() {
        match ops[i] {
            Op::If {
                address,
                width,
                test,
                value,
                then,
                otherwise,
            } => {
                let then_end = (i + 1 + then).min(ops.len());
                let else_end = (then_end + otherwise).min(ops.len());
                if test.holds(width, read(gba, address, width), value) {
                    run(&ops[i + 1..then_end], gba);
                } else {
                    run(&ops[then_end..else_end], gba);
                }
                i = else_end;
                continue;
            }
            Op::Write {
                address,
                width,
                value,
                count,
                address_step,
                value_step,
            } => {
                let (mut address, mut value) = (address, value);
                for _ in 0..count {
                    write(gba, address, width, value);
                    address = address.wrapping_add(address_step);
                    value = width.mask(value.wrapping_add(value_step));
                }
            }
            Op::WriteIndirect {
                pointer,
                offset,
                width,
                value,
            } => {
                let base = read(gba, pointer, Width::Word);
                write(gba, base.wrapping_add(offset), width, value);
            }
            Op::Modify {
                address,
                width,
                how,
                value,
            } => {
                let old = read(gba, address, width);
                let new = match how {
                    Modify::Add => old.wrapping_add(value),
                    Modify::Or => old | value,
                    Modify::And => old & value,
                };
                write(gba, address, width, width.mask(new));
            }
        }
        i += 1;
    }
}

fn read(gba: &Gba, address: u32, width: Width) -> u32 {
    match width {
        Width::Byte => u32::from(gba.bus.read8(address)),
        Width::Half => u32::from(gba.bus.read16(address)),
        Width::Word => gba.bus.read32(address),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn write(gba: &mut Gba, address: u32, width: Width, value: u32) {
    if matches!(
        MemoryRegion::from_address(address),
        None | Some(MemoryRegion::Bios | MemoryRegion::Rom)
    ) {
        return;
    }
    match width {
        Width::Byte => gba.bus.write8(address, value as u8),
        Width::Half => gba.bus.write16(address, value as u16),
        Width::Word => gba.bus.write32(address, value),
    }
}

/// Decodes every line as `format`.
fn decode(lines: &[&str], format: Format) -> Result<Cheat, CheatError> {
    let mut cheat = Cheat {
        format,
        ops: Vec::new(),
        skipped: Vec::new(),
    };
    match format {
        Format::Raw => {
            for (n, line) in lines.iter().enumerate() {
                let (address, value, width) =
                    raw_line(line).ok_or_else(|| shape(n, line, format))?;
                cheat.ops.push(Op::write(address, width, value));
            }
        }
        Format::GameShark => {
            let mut decoder = GameShark::default();
            for (n, line) in lines.iter().enumerate() {
                let (op1, op2) = pair_line(line).ok_or_else(|| shape(n, line, format))?;
                let (op1, op2) = decrypt(op1, op2, &GAMESHARK_SEEDS);
                decoder
                    .line(op1, op2, &mut cheat)
                    .map_err(|reason| CheatError::Unsupported {
                        line: n + 1,
                        reason,
                    })?;
            }
        }
        Format::ActionReplay => {
            let mut decoder = ActionReplay::default();
            for (n, line) in lines.iter().enumerate() {
                let (op1, op2) = pair_line(line).ok_or_else(|| shape(n, line, format))?;
                let (op1, op2) = decrypt(op1, op2, &ACTION_REPLAY_SEEDS);
                decoder
                    .line(op1, op2, &mut cheat)
                    .map_err(|reason| CheatError::Unsupported {
                        line: n + 1,
                        reason,
                    })?;
            }
            decoder.finish(&mut cheat);
        }
        Format::CodeBreaker => {
            let mut decoder = CodeBreaker::default();
            for (n, line) in lines.iter().enumerate() {
                let (op1, op2) = codebreaker_line(line).ok_or_else(|| shape(n, line, format))?;
                decoder
                    .line(op1, op2, &mut cheat)
                    .map_err(|reason| CheatError::Unsupported {
                        line: n + 1,
                        reason,
                    })?;
            }
        }
    }
    Ok(cheat)
}

fn shape(n: usize, line: &str, format: Format) -> CheatError {
    CheatError::Shape {
        line: n + 1,
        text: (*line).to_owned(),
        format,
    }
}

/// Hex digits only, at most eight of them.
fn hex(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 8 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

/// `AAAAAAAA:VV` with two, four or eight value digits.
fn raw_line(line: &str) -> Option<(u32, u32, Width)> {
    let (address, value) = line.split_once(':')?;
    let (address, value) = (address.trim(), value.trim());
    let width = match value.len() {
        2 => Width::Byte,
        4 => Width::Half,
        8 => Width::Word,
        _ => return None,
    };
    Some((hex(address)?, hex(value)?, width))
}

/// Two eight-digit halves, with or without the space between them.
fn pair_line(line: &str) -> Option<(u32, u32)> {
    let (a, b) = split_line(line, 8)?;
    Some((hex(a)?, hex(b)?))
}

/// Eight digits, then four.
fn codebreaker_line(line: &str) -> Option<(u32, u16)> {
    let (a, b) = split_line(line, 4)?;
    Some((hex(a)?, u16::try_from(hex(b)?).ok()?))
}

/// Splits `line` into an eight-digit and a `second`-digit part.
fn split_line(line: &str, second: usize) -> Option<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let (a, b) = match (parts.next(), parts.next(), parts.next()) {
        (Some(both), None, None) if both.len() == 8 + second => both.split_at(8),
        (Some(a), Some(b), None) => (a, b),
        _ => return None,
    };
    (a.len() == 8 && b.len() == second).then_some((a, b))
}

/// The GameShark v1/v2 TEA key.
const GAMESHARK_SEEDS: [u32; 4] = [0x09F4_FBBD, 0x9681_884A, 0x3520_27E9, 0xF3DE_E5A7];

/// The Action Replay v3 TEA key.
const ACTION_REPLAY_SEEDS: [u32; 4] = [0x7AA9_648F, 0x7FAE_6994, 0xC0EF_AAD5, 0x4271_2C57];

const TEA_DELTA: u32 = 0x9E37_79B9;

/// TEA decryption, 32 rounds, of one line.
fn decrypt(mut op1: u32, mut op2: u32, key: &[u32; 4]) -> (u32, u32) {
    let mut sum = TEA_DELTA.wrapping_mul(32);
    for _ in 0..32 {
        op2 = op2.wrapping_sub(
            (op1 << 4).wrapping_add(key[2])
                ^ op1.wrapping_add(sum)
                ^ (op1 >> 5).wrapping_add(key[3]),
        );
        op1 = op1.wrapping_sub(
            (op2 << 4).wrapping_add(key[0])
                ^ op2.wrapping_add(sum)
                ^ (op2 >> 5).wrapping_add(key[1]),
        );
        sum = sum.wrapping_sub(TEA_DELTA);
    }
    (op1, op2)
}

/// The second word of a device's "this is game X" line.
const ID_MARKER: u32 = 0x001D_C0DE;

/// Lines that change the decryption key for the lines after them.
const RESEED: u32 = 0xDEAD_FACE;

/// GameShark v1/v2 decoder state: the address list of a group write.
#[derive(Default)]
struct GameShark {
    /// Value and addresses still to come of a `3` code.
    group: Option<(u32, u32)>,
}

impl GameShark {
    fn line(&mut self, op1: u32, op2: u32, cheat: &mut Cheat) -> Result<(), String> {
        if let Some((value, left)) = &mut self.group {
            for address in [op1, op2].into_iter().take(*left as usize) {
                cheat.ops.push(Op::write(address, Width::Word, *value));
                *left -= 1;
            }
            if *left == 0 {
                self.group = None;
            }
            return Ok(());
        }
        if op2 == ID_MARKER {
            return Ok(());
        }
        let address = op1 & 0x0FFF_FFFF;
        match op1 >> 28 {
            0x0 => cheat.ops.push(Op::write(address, Width::Byte, op2)),
            0x1 => cheat.ops.push(Op::write(address, Width::Half, op2)),
            0x2 => cheat.ops.push(Op::write(address, Width::Word, op2)),
            0x3 => {
                let count = op1 & 0xFFFF;
                if count > 0 {
                    self.group = Some((op2, count));
                }
            }
            0x6 => cheat.skipped.push("a ROM patch".to_owned()),
            0x8 => cheat.skipped.push("a GameShark-button code".to_owned()),
            0xD if op1 == RESEED => return Err(reseeded()),
            0xD => {
                let test = match (op2 >> 20) & 0xF {
                    0 => Test::Eq,
                    1 => Test::Ne,
                    2 => Test::Ule,
                    3 => Test::Uge,
                    _ => return Err(format!("unknown GameShark condition {op1:08X} {op2:08X}")),
                };
                cheat
                    .ops
                    .push(Op::check(address, Width::Half, test, op2, 1));
            }
            0xE => {
                let lines = ((op1 >> 16) & 0xFF) as usize;
                let op = Op::check(op2 & 0x0FFF_FFFF, Width::Half, Test::Eq, op1, lines);
                cheat.ops.push(op);
            }
            // The hook: where the device patched itself into the game.
            0xF => {}
            _ => return Err(format!("unknown GameShark code {op1:08X} {op2:08X}")),
        }
        Ok(())
    }
}

fn reseeded() -> String {
    "re-keyed codes (DEADFACE) are not supported".to_owned()
}

/// What the line after an Action Replay "special" line is for.
enum ArPending {
    /// The value and step data of a fill (an index into the ops).
    Fill(usize),
    /// A line that belongs to something skipped.
    Skip,
}

/// Action Replay v3 decoder state.
#[derive(Default)]
struct ActionReplay {
    pending: Option<ArPending>,
    /// The open `if … [else …] endif` block: the index of its `If` op and
    /// where its else part starts.
    block: Option<(usize, Option<usize>)>,
}

/// Action Replay v3 packs addresses: region nibble in bits 20–23.
const fn ar_address(x: u32) -> u32 {
    (x & 0x000F_FFFF) | ((x << 4) & 0x0F00_0000)
}

impl ActionReplay {
    fn line(&mut self, op1: u32, op2: u32, cheat: &mut Cheat) -> Result<(), String> {
        match self.pending.take() {
            Some(ArPending::Skip) => return Ok(()),
            Some(ArPending::Fill(index)) => {
                if let Op::Write {
                    width,
                    value,
                    count,
                    address_step,
                    value_step,
                    ..
                } = &mut cheat.ops[index]
                {
                    *value = width.mask(op1);
                    *value_step = op2 >> 24;
                    *count = (op2 >> 16) & 0xFF;
                    *address_step = (op2 & 0xFFFF) * width.size();
                }
                return Ok(());
            }
            None => {}
        }
        if op2 == ID_MARKER {
            return Ok(());
        }
        if op1 == 0 {
            return self.special(op2, cheat);
        }
        if op1 == RESEED {
            return Err(reseeded());
        }
        // The hook.
        if op1 >> 24 == 0xC4 {
            return Ok(());
        }
        let width_bits = (op1 >> 25) & 3;
        if op1 & 0x3800_0000 != 0 {
            return self.condition(op1, op2, width_bits, cheat);
        }
        let unknown = || format!("unknown Action Replay code {op1:08X} {op2:08X}");
        let address = ar_address(op1);
        match op1 & 0xC000_0000 {
            0xC000_0000 => {
                // Writes to I/O: C6 is a halfword, C7 a word.
                let width = match op1 >> 24 {
                    0xC6 => Width::Half,
                    0xC7 => Width::Word,
                    _ => return Err(unknown()),
                };
                let address = 0x0400_0000 | (op1 & 0x00FF_FFFF);
                cheat.ops.push(Op::write(address, width, op2));
            }
            _ if op1 & 0x0100_0000 != 0 => return Err(unknown()),
            base => {
                let width = Width::bytes(1 << width_bits).ok_or_else(unknown)?;
                // Below a word, the bits above the value carry a count.
                let extra = if width == Width::Word {
                    0
                } else {
                    op2 >> (width.size() * 8)
                };
                let value = width.mask(op2);
                cheat.ops.push(match base {
                    0x0000_0000 => Op::Write {
                        address,
                        width,
                        value,
                        count: extra + 1,
                        address_step: width.size(),
                        value_step: 0,
                    },
                    0x4000_0000 => Op::WriteIndirect {
                        pointer: address,
                        offset: extra * width.size(),
                        width,
                        value,
                    },
                    _ => Op::Modify {
                        address,
                        width,
                        how: Modify::Add,
                        value,
                    },
                });
            }
        }
        Ok(())
    }

    fn condition(
        &mut self,
        op1: u32,
        op2: u32,
        width_bits: u32,
        cheat: &mut Cheat,
    ) -> Result<(), String> {
        let (width, test) = match Width::bytes(1 << width_bits) {
            Some(width) => (
                width,
                match op1 & 0x3800_0000 {
                    0x0800_0000 => Test::Eq,
                    0x1000_0000 => Test::Ne,
                    0x1800_0000 => Test::Lt,
                    0x2000_0000 => Test::Gt,
                    0x2800_0000 => Test::Ult,
                    0x3000_0000 => Test::Ugt,
                    _ => Test::Any,
                },
            ),
            // The "false" width: the condition never holds.
            None => (Width::Word, Test::Never),
        };
        let address = ar_address(op1);
        match op1 & 0xC000_0000 {
            0x0000_0000 => cheat.ops.push(Op::check(address, width, test, op2, 1)),
            0x4000_0000 => cheat.ops.push(Op::check(address, width, test, op2, 2)),
            0x8000_0000 => {
                self.end_block(cheat);
                self.block = Some((cheat.ops.len(), None));
                cheat.ops.push(Op::check(address, width, test, op2, 0));
            }
            _ => {
                return Err(
                    "Action Replay codes that turn other codes off are not supported".to_owned(),
                );
            }
        }
        Ok(())
    }

    /// A line whose first word is zero: the second says what it is.
    fn special(&mut self, op2: u32, cheat: &mut Cheat) -> Result<(), String> {
        let address = ar_address(op2);
        match op2 & 0xFF00_0000 {
            // End of the code list.
            0x0000_0000 => {}
            0x0800_0000 => cheat.skipped.push("a slowdown code".to_owned()),
            0x1000_0000 | 0x1200_0000 | 0x1400_0000 => {
                cheat
                    .skipped
                    .push("an Action Replay-button code".to_owned());
                self.pending = Some(ArPending::Skip);
            }
            0x1800_0000 | 0x1A00_0000 | 0x1C00_0000 | 0x1E00_0000 => {
                cheat.skipped.push("a ROM patch".to_owned());
                self.pending = Some(ArPending::Skip);
            }
            0x4000_0000 => self.end_block(cheat),
            0x6000_0000 => match &mut self.block {
                Some((_, otherwise @ None)) => *otherwise = Some(cheat.ops.len()),
                _ => return Err("an `else` outside an `if` block".to_owned()),
            },
            kind @ (0x8000_0000 | 0x8200_0000 | 0x8400_0000) => {
                let width = Width::bytes(1 << ((kind >> 25) & 3)).unwrap_or(Width::Word);
                self.pending = Some(ArPending::Fill(cheat.ops.len()));
                cheat.ops.push(Op::write(address, width, 0));
            }
            _ => return Err(format!("unknown Action Replay code 00000000 {op2:08X}")),
        }
        Ok(())
    }

    /// Closes the open block at the current end of the ops.
    fn end_block(&mut self, cheat: &mut Cheat) {
        let Some((start, otherwise_at)) = self.block.take() else {
            return;
        };
        let end = cheat.ops.len();
        let split = otherwise_at.unwrap_or(end);
        if let Op::If {
            then, otherwise, ..
        } = &mut cheat.ops[start]
        {
            *then = split - start - 1;
            *otherwise = end - split;
        }
    }

    fn finish(&mut self, cheat: &mut Cheat) {
        self.end_block(cheat);
    }
}

/// CodeBreaker decoder state.
#[derive(Default)]
struct CodeBreaker {
    pending: Option<CbPending>,
}

enum CbPending {
    /// The step line of a `4` fill, for the op at this index.
    Fill(usize),
    /// Halfwords of a `5` code still to come, and where the next goes.
    Bytes { address: u32, left: u32 },
}

impl CodeBreaker {
    fn line(&mut self, op1: u32, op2: u16, cheat: &mut Cheat) -> Result<(), String> {
        let value = u32::from(op2);
        match self.pending.take() {
            Some(CbPending::Fill(index)) => {
                if let Op::Write {
                    count,
                    address_step,
                    value_step,
                    ..
                } = &mut cheat.ops[index]
                {
                    *count = op1 & 0xFFFF;
                    *address_step = value;
                    *value_step = op1 >> 16;
                }
                return Ok(());
            }
            Some(CbPending::Bytes {
                mut address,
                mut left,
            }) => {
                // The data reads as bytes in memory order.
                let bytes = [op1.to_be_bytes().as_slice(), op2.to_be_bytes().as_slice()].concat();
                for pair in bytes.chunks(2).take(left as usize) {
                    let half = u32::from(u16::from_le_bytes([pair[0], pair[1]]));
                    cheat.ops.push(Op::write(address, Width::Half, half));
                    address += 2;
                    left -= 1;
                }
                if left > 0 {
                    self.pending = Some(CbPending::Bytes { address, left });
                }
                return Ok(());
            }
            None => {}
        }
        let address = op1 & 0x0FFF_FFFF;
        let modify = |how| Op::Modify {
            address,
            width: Width::Half,
            how,
            value,
        };
        let check = |test| Op::check(address, Width::Half, test, value, 1);
        match op1 >> 28 {
            // The game ID and the hook.
            0x0 | 0x1 => {}
            0x2 => cheat.ops.push(modify(Modify::Or)),
            0x3 => cheat.ops.push(Op::write(address, Width::Byte, value)),
            0x4 => {
                self.pending = Some(CbPending::Fill(cheat.ops.len()));
                cheat.ops.push(Op::write(address, Width::Half, value));
            }
            0x5 => {
                if value > 0 {
                    self.pending = Some(CbPending::Bytes {
                        address,
                        left: value,
                    });
                }
            }
            0x6 => cheat.ops.push(modify(Modify::And)),
            0x7 => cheat.ops.push(check(Test::Eq)),
            0x8 => cheat.ops.push(Op::write(address, Width::Half, value)),
            0x9 => return Err("encrypted CodeBreaker codes (type 9) are not supported".to_owned()),
            0xA => cheat.ops.push(check(Test::Ne)),
            0xB => cheat.ops.push(check(Test::Ugt)),
            0xC => cheat.ops.push(check(Test::Ult)),
            // "If these keys are held": KEYINPUT bits are low while held.
            0xD if address == 0x20 => {
                let keys = 0x0400_0000 | reg::KEYINPUT;
                cheat
                    .ops
                    .push(Op::check(keys, Width::Half, Test::None, value, 1));
            }
            0xD => return Err(format!("unknown CodeBreaker condition {op1:08X} {op2:04X}")),
            0xE => cheat.ops.push(modify(Modify::Add)),
            _ => cheat.ops.push(check(Test::Any)),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Cartridge;

    fn gba() -> Gba {
        let mut rom = vec![0u8; 0x200];
        rom[0xB2] = 0x96;
        Gba::new(Cartridge::from_bytes(rom).unwrap())
    }

    /// TEA encryption, for building test codes.
    fn encrypt(mut op1: u32, mut op2: u32, key: &[u32; 4]) -> (u32, u32) {
        let mut sum = 0u32;
        for _ in 0..32 {
            sum = sum.wrapping_add(TEA_DELTA);
            op1 = op1.wrapping_add(
                (op2 << 4).wrapping_add(key[0])
                    ^ op2.wrapping_add(sum)
                    ^ (op2 >> 5).wrapping_add(key[1]),
            );
            op2 = op2.wrapping_add(
                (op1 << 4).wrapping_add(key[2])
                    ^ op1.wrapping_add(sum)
                    ^ (op1 >> 5).wrapping_add(key[3]),
            );
        }
        (op1, op2)
    }

    fn encrypted(lines: &[(u32, u32)], key: &[u32; 4]) -> String {
        use std::fmt::Write;
        let mut text = String::new();
        for &(a, b) in lines {
            let (a, b) = encrypt(a, b, key);
            writeln!(text, "{a:08X} {b:08X}").unwrap();
        }
        text
    }

    fn run(text: &str, format: Option<Format>, gba: &mut Gba) -> Cheat {
        let cheat = Cheat::parse(text, format).unwrap();
        cheat.apply(gba);
        cheat
    }

    fn r8(gba: &Gba, a: u32) -> u32 {
        u32::from(gba.bus.read8(a))
    }

    fn r16(gba: &Gba, a: u32) -> u32 {
        u32::from(gba.bus.read16(a))
    }

    #[test]
    fn real_action_replay_master_codes_decrypt() {
        // Published "must be on" codes for Emerald and Ruby: the hook and
        // the game-ID line with its marker and the game code backwards.
        let (a, b) = decrypt(0xD8BA_E4D9, 0x4864_DCE5, &ACTION_REPLAY_SEEDS);
        assert_eq!((a >> 24, b), (0xC4, 0x8401));
        let (a, b) = decrypt(0xA86C_DBA5, 0x19BA_49B3, &ACTION_REPLAY_SEEDS);
        assert_eq!((&a.to_le_bytes(), b), (b"BPEE", ID_MARKER));
        let ruby = "DE00AAFD 2EBD05D0\n530823D9 16558191\n";
        let cheat = Cheat::parse(ruby, None).unwrap();
        assert_eq!(cheat.format(), Format::ActionReplay);
        assert!(cheat.is_empty(), "a master code does nothing by itself");
    }

    #[test]
    fn tea_round_trips_with_both_keys() {
        for key in [&GAMESHARK_SEEDS, &ACTION_REPLAY_SEEDS] {
            let (a, b) = encrypt(0x0200_1234, 0x63, key);
            assert_eq!(decrypt(a, b, key), (0x0200_1234, 0x63));
        }
    }

    #[test]
    fn raw_codes_write_at_their_width() {
        let mut gba = gba();
        let cheat = run(
            "02000000:12\n02000002:3456\n03000000:789ABCDE",
            None,
            &mut gba,
        );
        assert_eq!(cheat.format(), Format::Raw);
        assert_eq!(r8(&gba, 0x0200_0000), 0x12);
        assert_eq!(r16(&gba, 0x0200_0002), 0x3456);
        assert_eq!(gba.bus.read32(0x0300_0000), 0x789A_BCDE);
    }

    #[test]
    fn gameshark_codes_are_detected_and_applied() {
        let mut gba = gba();
        let text = encrypted(
            &[
                (0x0200_0010, 0x42),
                (0x1200_0012, 0xBEEF),
                // Group write of 0x11223344 to three addresses.
                (0x3000_0003, 0x1122_3344),
                (0x0300_0000, 0x0300_0004),
                (0x0300_0008, 0),
            ],
            &GAMESHARK_SEEDS,
        );
        let cheat = run(&text, None, &mut gba);
        assert_eq!(cheat.format(), Format::GameShark);
        assert_eq!(r8(&gba, 0x0200_0010), 0x42);
        assert_eq!(r16(&gba, 0x0200_0012), 0xBEEF);
        for a in [0x0300_0000, 0x0300_0004, 0x0300_0008] {
            assert_eq!(gba.bus.read32(a), 0x1122_3344);
        }
    }

    #[test]
    fn gameshark_conditions_guard_the_next_lines() {
        let mut gba = gba();
        let code = |cond: u32| {
            encrypted(
                &[
                    (0xD200_0000, cond),
                    (0x0200_0002, 0x77),
                    (0xE201_0005, 0x0200_0000),
                    (0x0200_0003, 0x88),
                    (0x0200_0004, 0x99),
                ],
                &GAMESHARK_SEEDS,
            )
        };
        // [0x02000000] is 0: "equal 0" holds, "equal 5" does not.
        run(&code(0x0000_0000), Some(Format::GameShark), &mut gba);
        assert_eq!(r8(&gba, 0x0200_0002), 0x77);
        assert_eq!(r8(&gba, 0x0200_0003), 0, "E code: 0 != 5");
        assert_eq!(r8(&gba, 0x0200_0004), 0x99, "only two lines guarded");
        let mut gba = self::gba();
        run(&code(0x0010_0000), Some(Format::GameShark), &mut gba);
        assert_eq!(r8(&gba, 0x0200_0002), 0, "not-equal 0 fails");
    }

    /// Action Replay instruction tests on plain (decrypted) lines.
    fn ar_raw(lines: &[(u32, u32)], gba: &mut Gba) -> Cheat {
        run(
            &encrypted(lines, &ACTION_REPLAY_SEEDS),
            Some(Format::ActionReplay),
            gba,
        )
    }

    #[test]
    fn action_replay_writes_fills_and_adds() {
        let mut gba = gba();
        ar_raw(
            &[
                (0x0030_0000, 0x78),
                (0x0230_0002, 0x5678),
                (0x0430_0004, 0x1234_5678),
                // 8-bit assign repeated three times.
                (0x0020_0000, 0x0000_0207),
                // Fill: value 1 (+1 each), two units, every second byte.
                (0, 0x8030_0010),
                (1, 0x0102_0002),
                (0x8030_0004, 2),
            ],
            &mut gba,
        );
        assert_eq!(r8(&gba, 0x0300_0000), 0x78);
        assert_eq!(r16(&gba, 0x0300_0002), 0x5678);
        assert_eq!(
            gba.bus.read32(0x0300_0004),
            0x1234_567A,
            "add after the write"
        );
        for a in 0..3 {
            assert_eq!(r8(&gba, 0x0200_0000 + a), 7);
        }
        assert_eq!(r8(&gba, 0x0200_0003), 0);
        assert_eq!(r8(&gba, 0x0300_0010), 1);
        assert_eq!(r8(&gba, 0x0300_0011), 0);
        assert_eq!(r8(&gba, 0x0300_0012), 2);
    }

    #[test]
    fn action_replay_blocks_with_else() {
        let block = |value: u32| {
            let mut gba = gba();
            gba.bus.write8(0x0300_0000, 1);
            ar_raw(
                &[
                    (0x8830_0000, value),
                    (0x0030_0001, 0x12),
                    (0, 0x6000_0000),
                    (0x0030_0002, 0x22),
                    (0, 0x4000_0000),
                    (0x0030_0003, 0x33),
                ],
                &mut gba,
            );
            (
                r8(&gba, 0x0300_0001),
                r8(&gba, 0x0300_0002),
                r8(&gba, 0x0300_0003),
            )
        };
        assert_eq!(block(1), (0x12, 0, 0x33));
        assert_eq!(block(2), (0, 0x22, 0x33));
    }

    #[test]
    fn action_replay_skips_what_cannot_work_here() {
        let mut gba = gba();
        let cheat = ar_raw(
            &[(0, 0x1800_1234), (0x1234_5678, 0), (0x0030_0000, 9)],
            &mut gba,
        );
        assert_eq!(cheat.skipped(), ["a ROM patch"]);
        assert_eq!(
            r8(&gba, 0x0300_0000),
            9,
            "the line after the patch data still runs"
        );
    }

    #[test]
    fn codebreaker_codes() {
        let mut gba = gba();
        let cheat = run(
            "32000000 0012\n\
             82000002 BEEF\n\
             E2000002 0001\n\
             42000010 0005\n\
             00010002 0004\n\
             52000020 0002\n\
             11223344 0000\n",
            None,
            &mut gba,
        );
        assert_eq!(cheat.format(), Format::CodeBreaker);
        assert_eq!(r8(&gba, 0x0200_0000), 0x12);
        assert_eq!(r16(&gba, 0x0200_0002), 0xBEF0);
        assert_eq!(r16(&gba, 0x0200_0010), 5);
        assert_eq!(r16(&gba, 0x0200_0012), 0, "a four-byte step");
        assert_eq!(r16(&gba, 0x0200_0014), 6);
        assert_eq!(r8(&gba, 0x0200_0020), 0x11, "bytes in memory order");
        assert_eq!(r8(&gba, 0x0200_0023), 0x44);
    }

    #[test]
    fn codebreaker_key_condition_follows_keyinput() {
        let code = "D0000020 0008\n32000000 0001\n";
        let mut gba = gba();
        run(code, None, &mut gba);
        assert_eq!(r8(&gba, 0x0200_0000), 0, "Start not held");
        gba.set_keyinput(0x03FF & !0x0008);
        run(code, None, &mut gba);
        assert_eq!(r8(&gba, 0x0200_0000), 1);
    }

    #[test]
    fn errors_name_the_problem() {
        assert_eq!(Cheat::parse(" \n", None), Err(CheatError::Empty));
        assert_eq!(Cheat::parse("hello", None), Err(CheatError::Unrecognised));
        let err = Cheat::parse("92000000 1234", None).unwrap_err();
        assert!(err.to_string().contains("type 9"), "{err}");
        let err = Cheat::parse("0200:12", Some(Format::GameShark)).unwrap_err();
        assert!(matches!(err, CheatError::Shape { line: 1, .. }), "{err}");
    }

    #[test]
    fn cheats_never_write_the_rom() {
        let mut gba = gba();
        run("08000000:FF", None, &mut gba);
        assert_eq!(r8(&gba, 0x0800_0000), 0);
    }

    #[test]
    fn format_names_round_trip() {
        for format in Format::ALL {
            assert_eq!(Format::from_name(format.name()), Some(format));
        }
    }
}
