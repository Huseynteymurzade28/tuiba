//! Memory-mapped I/O registers (`0x0400_0000..0x0400_0400`).
//!
//! This is scaffolding: registers are backed by a raw byte array so that
//! software can store and read back values, plus the handful of semantics
//! that are cheap and easy to get right now (read-only registers,
//! write-to-clear `IF`, live `KEYINPUT`). Hardware units (PPU, timers, DMA,
//! interrupts) will take ownership of their registers as they are built.

use crate::memory::IO_SIZE;

/// Register offsets relative to the I/O base address.
#[allow(missing_docs)]
pub mod reg {
    // LCD
    pub const DISPCNT: u32 = 0x000;
    pub const DISPSTAT: u32 = 0x004;
    pub const VCOUNT: u32 = 0x006;
    pub const BG0CNT: u32 = 0x008;
    pub const BG1CNT: u32 = 0x00A;
    pub const BG2CNT: u32 = 0x00C;
    pub const BG3CNT: u32 = 0x00E;
    pub const BG0HOFS: u32 = 0x010;
    pub const BG0VOFS: u32 = 0x012;
    pub const BG1HOFS: u32 = 0x014;
    pub const BG1VOFS: u32 = 0x016;
    pub const BG2HOFS: u32 = 0x018;
    pub const BG2VOFS: u32 = 0x01A;
    pub const BG3HOFS: u32 = 0x01C;
    pub const BG3VOFS: u32 = 0x01E;

    // Sound (not emulated; stored so games see their writes)
    pub const SOUNDCNT_L: u32 = 0x080;
    pub const SOUNDCNT_H: u32 = 0x082;
    pub const SOUNDCNT_X: u32 = 0x084;
    pub const SOUNDBIAS: u32 = 0x088;

    // DMA
    pub const DMA0SAD: u32 = 0x0B0;
    pub const DMA0CNT_H: u32 = 0x0BA;
    pub const DMA3CNT_H: u32 = 0x0DE;

    // Timers
    pub const TM0CNT_L: u32 = 0x100;
    pub const TM0CNT_H: u32 = 0x102;
    pub const TM3CNT_H: u32 = 0x10E;

    // Keypad
    pub const KEYINPUT: u32 = 0x130;
    pub const KEYCNT: u32 = 0x132;

    // Interrupts, wait-state and power control
    pub const IE: u32 = 0x200;
    pub const IF: u32 = 0x202;
    pub const WAITCNT: u32 = 0x204;
    pub const IME: u32 = 0x208;
    pub const POSTFLG: u32 = 0x300;
    pub const HALTCNT: u32 = 0x301;
}

/// `KEYINPUT` value with every key released (bits are active-low).
pub const KEYINPUT_ALL_RELEASED: u16 = 0x03FF;

/// The I/O register block.
#[derive(Debug, Clone)]
pub struct IoRegisters {
    raw: Box<[u8]>,
    /// Current `KEYINPUT` state, written by the frontend. `0` = pressed.
    pub keyinput: u16,
}

impl Default for IoRegisters {
    fn default() -> Self {
        Self::new()
    }
}

impl IoRegisters {
    /// Creates a register block in its power-on state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            raw: vec![0; IO_SIZE].into_boxed_slice(),
            keyinput: KEYINPUT_ALL_RELEASED,
        }
    }

    /// Reads a halfword register. `offset` is masked to the register block
    /// and forced even; offsets beyond the block read as `0`.
    #[must_use]
    pub fn read16(&self, offset: u32) -> u16 {
        let off = (offset & !1) as usize;
        if off >= IO_SIZE {
            return 0;
        }
        match off as u32 {
            reg::KEYINPUT => self.keyinput,
            _ => u16::from_le_bytes([self.raw[off], self.raw[off + 1]]),
        }
    }

    /// Reads a byte register.
    #[must_use]
    pub fn read8(&self, offset: u32) -> u8 {
        (self.read16(offset) >> ((offset & 1) * 8)) as u8
    }

    /// Reads a word as two consecutive halfword registers.
    #[must_use]
    pub fn read32(&self, offset: u32) -> u32 {
        let aligned = offset & !3;
        u32::from(self.read16(aligned)) | (u32::from(self.read16(aligned + 2)) << 16)
    }

    /// Writes a halfword register, applying per-register semantics.
    pub fn write16(&mut self, offset: u32, value: u16) {
        let off = (offset & !1) as usize;
        if off >= IO_SIZE {
            return;
        }
        let value = match off as u32 {
            // Read-only.
            reg::VCOUNT | reg::KEYINPUT => return,
            // Writing `1` acknowledges (clears) the corresponding bit.
            reg::IF => self.read16(reg::IF) & !value,
            _ => value,
        };
        self.raw[off..off + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a byte register by merging it into the containing halfword.
    pub fn write8(&mut self, offset: u32, value: u8) {
        let aligned = offset & !1;
        let shift = (offset & 1) * 8;
        let half = match aligned {
            // Merging would re-acknowledge bits in the other byte.
            reg::IF => u16::from(value) << shift,
            _ => (self.read16(aligned) & !(0xFF << shift)) | (u16::from(value) << shift),
        };
        self.write16(aligned, half);
    }

    /// Writes a word as two consecutive halfword registers.
    pub fn write32(&mut self, offset: u32, value: u32) {
        let aligned = offset & !3;
        self.write16(aligned, value as u16);
        self.write16(aligned + 2, (value >> 16) as u16);
    }

    /// Sets a register's stored value directly, bypassing write semantics.
    ///
    /// Intended for hardware units updating their own read-only registers
    /// (e.g. the PPU advancing `VCOUNT`).
    pub fn set_raw16(&mut self, offset: u32, value: u16) {
        let off = (offset & !1) as usize;
        if off < IO_SIZE {
            self.raw[off..off + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reads_back() {
        let mut io = IoRegisters::new();
        io.write16(reg::DISPCNT, 0x0403);
        assert_eq!(io.read16(reg::DISPCNT), 0x0403);
        assert_eq!(io.read8(reg::DISPCNT), 0x03);
        assert_eq!(io.read8(reg::DISPCNT + 1), 0x04);
    }

    #[test]
    fn word_access_spans_two_registers() {
        let mut io = IoRegisters::new();
        io.write32(reg::IE, 0x0002_0001);
        assert_eq!(io.read16(reg::IE), 0x0001);
        assert_eq!(
            io.read16(reg::IF),
            0x0000,
            "IF write-to-clear must not set bits"
        );
        io.set_raw16(reg::IF, 0x0002);
        assert_eq!(io.read32(reg::IE), 0x0002_0001);
    }

    #[test]
    fn byte_write_merges_into_halfword() {
        let mut io = IoRegisters::new();
        io.write16(reg::BG0CNT, 0x1234);
        io.write8(reg::BG0CNT + 1, 0xAB);
        assert_eq!(io.read16(reg::BG0CNT), 0xAB34);
    }

    #[test]
    fn keyinput_is_live_and_read_only() {
        let mut io = IoRegisters::new();
        assert_eq!(io.read16(reg::KEYINPUT), KEYINPUT_ALL_RELEASED);
        io.write16(reg::KEYINPUT, 0);
        assert_eq!(io.read16(reg::KEYINPUT), KEYINPUT_ALL_RELEASED);
        io.keyinput = 0x03FE;
        assert_eq!(io.read16(reg::KEYINPUT), 0x03FE);
    }

    #[test]
    fn vcount_is_read_only_but_settable_by_hardware() {
        let mut io = IoRegisters::new();
        io.write16(reg::VCOUNT, 99);
        assert_eq!(io.read16(reg::VCOUNT), 0);
        io.set_raw16(reg::VCOUNT, 99);
        assert_eq!(io.read16(reg::VCOUNT), 99);
    }

    #[test]
    fn if_is_write_to_clear() {
        let mut io = IoRegisters::new();
        io.set_raw16(reg::IF, 0b0111);
        io.write16(reg::IF, 0b0010);
        assert_eq!(io.read16(reg::IF), 0b0101);
        io.write8(reg::IF, 0b0001);
        assert_eq!(io.read16(reg::IF), 0b0100);
    }

    #[test]
    fn out_of_block_offsets_are_inert() {
        let mut io = IoRegisters::new();
        io.write16(0x800, 0xFFFF);
        assert_eq!(io.read16(0x800), 0);
    }
}
