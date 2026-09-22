//! Memory-mapped I/O registers (`0x0400_0000..0x0400_0400`).
//!
//! This is scaffolding: registers are backed by a raw byte array so that
//! software can store and read back values, plus the handful of semantics
//! that are cheap and easy to get right now (read-only registers,
//! write-to-clear `IF`, live `KEYINPUT`). Hardware units (PPU, timers, DMA,
//! interrupts) will take ownership of their registers as they are built.

use crate::apu::Apu;
use crate::memory::IO_SIZE;
use crate::memory::dma::Dma;
use crate::memory::timers::Timers;

/// First and last DMA register offsets.
const DMA_RANGE: std::ops::RangeInclusive<usize> = 0x0B0..=0x0DE;
/// First and last timer register offsets.
const TIMER_RANGE: std::ops::RangeInclusive<usize> = 0x100..=0x10E;

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
    pub const BG2PA: u32 = 0x020;
    pub const BG2PB: u32 = 0x022;
    pub const BG2PC: u32 = 0x024;
    pub const BG2PD: u32 = 0x026;
    pub const BG2X: u32 = 0x028;
    pub const BG2Y: u32 = 0x02C;
    pub const BG3PA: u32 = 0x030;
    pub const BG3PB: u32 = 0x032;
    pub const BG3PC: u32 = 0x034;
    pub const BG3PD: u32 = 0x036;
    pub const BG3X: u32 = 0x038;
    pub const BG3Y: u32 = 0x03C;
    pub const WIN0H: u32 = 0x040;
    pub const WIN1H: u32 = 0x042;
    pub const WIN0V: u32 = 0x044;
    pub const WIN1V: u32 = 0x046;
    pub const WININ: u32 = 0x048;
    pub const WINOUT: u32 = 0x04A;
    pub const MOSAIC: u32 = 0x04C;
    pub const BLDCNT: u32 = 0x050;
    pub const BLDALPHA: u32 = 0x052;
    pub const BLDY: u32 = 0x054;

    // Sound: see `crate::apu::reg` for the full map.
    pub use crate::apu::reg::{SOUNDBIAS, SOUNDCNT_H, SOUNDCNT_L, SOUNDCNT_X};

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

/// Interrupt sources, numbered by their bit in `IE`/`IF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum Interrupt {
    VBlank = 0,
    HBlank = 1,
    VCount = 2,
    Timer0 = 3,
    Timer1 = 4,
    Timer2 = 5,
    Timer3 = 6,
    Serial = 7,
    Dma0 = 8,
    Dma1 = 9,
    Dma2 = 10,
    Dma3 = 11,
    Keypad = 12,
    GamePak = 13,
}

impl Interrupt {
    /// The `IE`/`IF` bit for this source.
    #[must_use]
    pub const fn mask(self) -> u16 {
        1 << (self as u16)
    }
}

/// The I/O register block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IoRegisters {
    raw: Box<[u8]>,
    /// Current `KEYINPUT` state, written by the frontend. `0` = pressed.
    pub keyinput: u16,
    /// Set by a write to `HALTCNT`; the emulator takes it and halts the CPU.
    pub halt_requested: bool,
    /// The four timers.
    pub timers: Timers,
    /// The DMA controller registers.
    pub dma: Dma,
    /// The sound unit.
    pub apu: Apu,
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
            halt_requested: false,
            timers: Timers::new(),
            dma: Dma::new(),
            apu: Apu::new(),
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
        if crate::apu::REGISTER_RANGE.contains(&(off as u32)) {
            return self.apu.read16(off as u32);
        }
        if DMA_RANGE.contains(&off) {
            let (n, sub) = ((off - 0xB0) / 12, (off - 0xB0) % 12);
            // Only the control halfword is readable.
            return if sub == 10 {
                self.dma.channels[n].control
            } else {
                0
            };
        }
        if TIMER_RANGE.contains(&off) {
            let (n, sub) = ((off - 0x100) / 4, (off - 0x100) % 4);
            return if sub == 0 {
                self.timers.counter(n)
            } else {
                self.timers.control(n)
            };
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
        if crate::apu::REGISTER_RANGE.contains(&(off as u32)) {
            self.apu.write16(off as u32, value);
            return;
        }
        if DMA_RANGE.contains(&off) {
            let (n, sub) = ((off - 0xB0) / 12, (off - 0xB0) % 12);
            let ch = &mut self.dma.channels[n];
            match sub {
                0 => ch.source = (ch.source & 0xFFFF_0000) | u32::from(value),
                2 => ch.source = (ch.source & 0xFFFF) | (u32::from(value) << 16),
                4 => ch.dest = (ch.dest & 0xFFFF_0000) | u32::from(value),
                6 => ch.dest = (ch.dest & 0xFFFF) | (u32::from(value) << 16),
                8 => ch.count = value,
                _ => self.dma.write_control(n, value),
            }
            return;
        }
        if TIMER_RANGE.contains(&off) {
            let (n, sub) = ((off - 0x100) / 4, (off - 0x100) % 4);
            if sub == 0 {
                self.timers.set_reload(n, value);
            } else {
                self.timers.set_control(n, value);
            }
            return;
        }
        let value = match off as u32 {
            // Read-only.
            reg::VCOUNT | reg::KEYINPUT => return,
            // Bits 2:0 are status flags owned by the PPU.
            reg::DISPSTAT => (value & !0x7) | (self.read16(reg::DISPSTAT) & 0x7),
            // Writing `1` acknowledges (clears) the corresponding bit.
            reg::IF => self.read16(reg::IF) & !value,
            _ => value,
        };
        self.raw[off..off + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a byte register by merging it into the containing halfword.
    pub fn write8(&mut self, offset: u32, value: u8) {
        // HALTCNT: bit 7 clear = halt, set = stop (treated as halt).
        if offset == reg::HALTCNT {
            self.halt_requested = true;
            return;
        }
        if crate::apu::REGISTER_RANGE.contains(&offset) {
            // Sound registers have write-only fields that a read-merge
            // would lose, and FIFO bytes are pushed rather than stored.
            self.apu.write8(offset, value);
            return;
        }
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

    /// Raises `source` in `IF`. Whether it reaches the CPU depends on `IE`
    /// and `IME`, which the interrupt controller checks separately.
    pub fn request_interrupt(&mut self, source: Interrupt) {
        let current = self.read16(reg::IF);
        self.set_raw16(reg::IF, current | source.mask());
    }

    /// Whether an interrupt is pending and enabled: `IME`, `IE & IF`.
    #[must_use]
    pub fn irq_pending(&self) -> bool {
        self.read16(reg::IME) & 1 != 0 && self.read16(reg::IE) & self.read16(reg::IF) != 0
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
    fn dispstat_flags_survive_software_writes() {
        let mut io = IoRegisters::new();
        io.set_raw16(reg::DISPSTAT, 0b011);
        io.write16(reg::DISPSTAT, 0x1F38);
        assert_eq!(io.read16(reg::DISPSTAT), 0x1F3B);
    }

    #[test]
    fn interrupt_request_and_pending() {
        let mut io = IoRegisters::new();
        io.request_interrupt(Interrupt::VBlank);
        io.request_interrupt(Interrupt::Timer1);
        assert_eq!(io.read16(reg::IF), 0b1_0001);
        assert!(!io.irq_pending(), "IME off");
        io.write16(reg::IME, 1);
        assert!(!io.irq_pending(), "IE empty");
        io.write16(reg::IE, Interrupt::Timer1.mask());
        assert!(io.irq_pending());
        io.write16(reg::IF, Interrupt::Timer1.mask());
        assert!(!io.irq_pending(), "acknowledged");
    }

    #[test]
    fn haltcnt_requests_halt() {
        let mut io = IoRegisters::new();
        io.write8(reg::HALTCNT, 0);
        assert!(io.halt_requested);
        io.halt_requested = false;
        io.write16(reg::POSTFLG, 0x0001);
        assert!(
            !io.halt_requested,
            "halfword write to POSTFLG is not a halt"
        );
    }

    #[test]
    fn dma_registers_route_to_channels() {
        let mut io = IoRegisters::new();
        io.write32(reg::DMA0SAD + 12 * 3, 0x0800_1234); // DMA3SAD
        io.write32(reg::DMA0SAD + 12 * 3 + 4, 0x0600_0000); // DMA3DAD
        io.write16(reg::DMA0SAD + 12 * 3 + 8, 0x100); // DMA3CNT_L
        assert_eq!(io.dma.channels[3].source, 0x0800_1234);
        assert_eq!(io.dma.channels[3].dest, 0x0600_0000);
        assert_eq!(io.dma.channels[3].count, 0x100);
        assert_eq!(io.read32(reg::DMA0SAD + 12 * 3), 0, "SAD is write-only");
        io.write16(reg::DMA3CNT_H, 0x8400);
        assert_eq!(io.read16(reg::DMA3CNT_H), 0x8400);
        assert_eq!(io.dma.take_pending(), 0b1000);
    }

    #[test]
    fn timer_registers_route_to_timers() {
        let mut io = IoRegisters::new();
        io.write16(reg::TM0CNT_L, 0xFF00);
        assert_eq!(
            io.read16(reg::TM0CNT_L),
            0,
            "reload is not visible until enabled"
        );
        io.write16(reg::TM0CNT_H, 0x80);
        assert_eq!(io.read16(reg::TM0CNT_L), 0xFF00);
        assert_eq!(io.read16(reg::TM0CNT_H), 0x80);
        io.write32(reg::TM3CNT_H - 2, 0x00C0_1234); // TM3CNT_L=0x1234, TM3CNT_H=0xC0
        assert_eq!(io.read16(reg::TM3CNT_H), 0xC0);
        assert_eq!(io.timers.counter(3), 0x1234);
    }

    #[test]
    fn out_of_block_offsets_are_inert() {
        let mut io = IoRegisters::new();
        io.write16(0x800, 0xFFFF);
        assert_eq!(io.read16(0x800), 0);
    }
}
