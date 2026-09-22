//! DMA controller registers and transfer engine.
//!
//! Register state (source, destination, count, control) lives here. The
//! transfer itself needs the whole bus, so it is a free function the
//! system calls when a channel is triggered.

use crate::apu::reg::{FIFO_A, FIFO_B};
use crate::memory::Memory;
use crate::memory::base;
use crate::memory::io::Interrupt;

/// Words a sound FIFO refill moves, whatever the channel's count says.
const FIFO_TRANSFER_WORDS: u32 = 4;

/// When a channel starts its transfer (`DMAxCNT_H` bits 13:12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Timing {
    /// As soon as it is enabled.
    Immediate,
    /// At the start of each VBlank.
    VBlank,
    /// At the start of each visible line's HBlank.
    HBlank,
    /// Sound FIFO refill (channels 1–2) or video capture (channel 3; not
    /// emulated).
    Special,
}

/// One DMA channel.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    /// `DMAxSAD` as written.
    pub source: u32,
    /// `DMAxDAD` as written.
    pub dest: u32,
    /// `DMAxCNT_L` as written.
    pub count: u16,
    /// `DMAxCNT_H`.
    pub control: u16,
    /// Internal source address, latched on enable.
    latched_source: u32,
    /// Internal destination address, latched on enable / reload.
    latched_dest: u32,
}

impl Channel {
    const ENABLE: u16 = 1 << 15;
    const IRQ: u16 = 1 << 14;
    const REPEAT: u16 = 1 << 9;
    const WORD: u16 = 1 << 10;

    /// The `(source, destination)` the next transfer will start from.
    #[must_use]
    pub fn latched_addresses(&self) -> (u32, u32) {
        (self.latched_source, self.latched_dest)
    }

    /// Whether the channel is armed.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.control & Self::ENABLE != 0
    }

    /// The trigger condition.
    #[must_use]
    pub fn timing(&self) -> Timing {
        match (self.control >> 12) & 0b11 {
            0 => Timing::Immediate,
            1 => Timing::VBlank,
            2 => Timing::HBlank,
            _ => Timing::Special,
        }
    }

    /// Number of units to transfer; `0` means the maximum.
    #[must_use]
    pub fn unit_count(&self, channel: usize) -> u32 {
        let max = if channel == 3 { 0x1_0000 } else { 0x4000 };
        if self.count == 0 {
            max
        } else {
            u32::from(self.count)
        }
    }
}

/// The four channels plus the "ready to run" bookkeeping.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Dma {
    /// Channels 0–3.
    pub channels: [Channel; 4],
    /// Bit mask of channels that must run before the CPU continues.
    pending: u8,
}

impl Dma {
    /// All channels idle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes `DMAxCNT_H`. A rising enable bit latches the addresses and,
    /// for immediate timing, schedules the transfer.
    pub fn write_control(&mut self, n: usize, value: u16) {
        let ch = &mut self.channels[n];
        let was_enabled = ch.enabled();
        ch.control = value & if n == 0 { 0xF7E0 } else { 0xFFE0 };
        if !was_enabled && ch.enabled() {
            ch.latched_source = ch.source & if n == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF };
            ch.latched_dest = ch.dest & if n == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF };
            if ch.timing() == Timing::Immediate {
                self.pending |= 1 << n;
            }
        }
    }

    /// Schedules every enabled channel with the given timing.
    pub fn trigger(&mut self, timing: Timing) {
        for (n, ch) in self.channels.iter().enumerate() {
            if ch.enabled() && ch.timing() == timing {
                self.pending |= 1 << n;
            }
        }
    }

    /// Schedules the refill of sound FIFO `fifo` (0 = A, 1 = B): channels
    /// 1 and 2 in special timing whose destination is that FIFO.
    pub fn trigger_fifo(&mut self, fifo: usize) {
        let target = base::IO + if fifo == 0 { FIFO_A } else { FIFO_B };
        for n in 1..=2 {
            let ch = &self.channels[n];
            if ch.enabled() && ch.timing() == Timing::Special && ch.latched_dest == target {
                self.pending |= 1 << n;
            }
        }
    }

    /// Takes the bit mask of channels waiting to run (bit `n` = channel `n`).
    pub fn take_pending(&mut self) -> u8 {
        std::mem::take(&mut self.pending)
    }
}

/// Performs channel `n`'s transfer on `mem`. Returns the interrupt to raise
/// on completion, if the channel asks for one.
pub fn run(dma: &mut Dma, n: usize, mem: &mut impl Memory) -> Option<Interrupt> {
    let ch = &mut dma.channels[n];
    if !ch.enabled() {
        return None;
    }
    let fifo = ch.timing() == Timing::Special;
    if fifo && n == 3 {
        // Video capture: not emulated. Leave the channel armed so software
        // sees it as running, but move nothing.
        return None;
    }

    // A FIFO refill is always four words to a fixed destination.
    let word = fifo || ch.control & Channel::WORD != 0;
    let unit = if word { 4 } else { 2 };
    let count = if fifo {
        FIFO_TRANSFER_WORDS
    } else {
        ch.unit_count(n)
    };
    let src_ctrl = (ch.control >> 7) & 0b11;
    let dst_ctrl = if fifo { 2 } else { (ch.control >> 5) & 0b11 };

    let mut src = ch.latched_source & !(unit - 1);
    let mut dst = ch.latched_dest & !(unit - 1);
    for _ in 0..count {
        if word {
            mem.write32(dst, mem.read32(src));
        } else {
            mem.write16(dst, mem.read16(src));
        }
        src = step(src, src_ctrl, unit);
        dst = step(dst, dst_ctrl, unit);
    }
    ch.latched_source = src;

    if fifo || ch.control & Channel::REPEAT != 0 && ch.timing() != Timing::Immediate {
        // Repeat: keep running on later triggers; dest reloads with mode 3.
        ch.latched_dest = if dst_ctrl == 3 { ch.dest } else { dst };
    } else {
        ch.latched_dest = dst;
        ch.control &= !Channel::ENABLE;
    }

    (ch.control & Channel::IRQ != 0).then_some(match n {
        0 => Interrupt::Dma0,
        1 => Interrupt::Dma1,
        2 => Interrupt::Dma2,
        _ => Interrupt::Dma3,
    })
}

/// Applies an address-control mode: 0 = increment, 1 = decrement,
/// 2 = fixed, 3 = increment (destination reload is handled by the caller).
#[inline]
fn step(address: u32, control: u16, unit: u32) -> u32 {
    match control {
        1 => address.wrapping_sub(unit),
        2 => address,
        _ => address.wrapping_add(unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::test_util::Ram;

    fn armed(n: usize, src: u32, dst: u32, count: u16, control: u16) -> Dma {
        let mut dma = Dma::new();
        dma.channels[n].source = src;
        dma.channels[n].dest = dst;
        dma.channels[n].count = count;
        dma.write_control(n, control);
        dma
    }

    #[test]
    fn immediate_halfword_copy() {
        let mut mem = Ram::new();
        for i in 0..4u32 {
            mem.write16(0x1000 + i * 2, 0x1111 * (i as u16 + 1));
        }
        let mut dma = armed(3, 0x1000, 0x2000, 4, 0x8000);
        assert_eq!(dma.take_pending(), 0b1000);
        assert_eq!(run(&mut dma, 3, &mut mem), None);
        assert_eq!(mem.read16(0x2006), 0x4444);
        assert!(!dma.channels[3].enabled(), "cleared when done");
    }

    #[test]
    fn word_fill_with_fixed_source_and_irq() {
        let mut mem = Ram::new();
        mem.write32(0x1000, 0xDEAD_BEEF);
        // word, src fixed (2 << 7), IRQ, enable
        let mut dma = armed(1, 0x1000, 0x2000, 3, 0x8000 | 0x4000 | 0x0400 | (2 << 7));
        assert_eq!(run(&mut dma, 1, &mut mem), Some(Interrupt::Dma1));
        assert_eq!(mem.read32(0x2000), 0xDEAD_BEEF);
        assert_eq!(mem.read32(0x2008), 0xDEAD_BEEF);
        assert_eq!(mem.read32(0x200C), 0);
    }

    #[test]
    fn zero_count_means_maximum() {
        let mut dma = armed(0, 0, 0, 0, 0x8000);
        assert_eq!(dma.channels[0].unit_count(0), 0x4000);
        let dma3 = armed(3, 0, 0, 0, 0x8000);
        assert_eq!(dma3.channels[3].unit_count(3), 0x1_0000);
        let _ = dma.take_pending();
    }

    #[test]
    fn hblank_repeat_reloads_destination() {
        let mut mem = Ram::new();
        mem.write16(0x1000, 0xAAAA);
        mem.write16(0x1002, 0xBBBB);
        // HBlank timing, repeat, dest inc/reload (3 << 5)
        let mut dma = armed(0, 0x1000, 0x2000, 1, 0x8000 | 0x0200 | (2 << 12) | (3 << 5));
        assert_eq!(dma.take_pending(), 0, "not immediate");
        dma.trigger(Timing::HBlank);
        assert_eq!(dma.take_pending(), 0b0001);
        run(&mut dma, 0, &mut mem);
        assert_eq!(mem.read16(0x2000), 0xAAAA);
        assert!(dma.channels[0].enabled(), "repeat keeps it armed");
        dma.trigger(Timing::HBlank);
        let _ = dma.take_pending();
        run(&mut dma, 0, &mut mem);
        assert_eq!(mem.read16(0x2000), 0xBBBB, "dest reloaded, source advanced");
    }

    #[test]
    fn fifo_refill_moves_four_words_to_a_fixed_destination() {
        let mut mem = Ram::new();
        for i in 0..8u32 {
            mem.write32(0x1000 + i * 4, 0x1111_1111 * (i + 1));
        }
        // Special timing, repeat, dest increment (ignored), halfwords
        // (ignored), count 1 (ignored).
        let fifo_a = base::IO + FIFO_A;
        let mut dma = armed(1, 0x1000, fifo_a, 1, 0x8000 | 0x0200 | (3 << 12));
        dma.trigger_fifo(1);
        assert_eq!(dma.take_pending(), 0, "FIFO B is not this channel's target");
        dma.trigger_fifo(0);
        assert_eq!(dma.take_pending(), 0b0010);
        run(&mut dma, 1, &mut mem);
        // Ram is flat, so the fixed destination ends up holding the last
        // word written.
        assert_eq!(mem.read32(fifo_a), 0x4444_4444);
        assert_eq!(mem.read32(fifo_a + 4), 0, "destination did not advance");
        assert!(dma.channels[1].enabled(), "stays armed");
        dma.trigger_fifo(0);
        let _ = dma.take_pending();
        run(&mut dma, 1, &mut mem);
        assert_eq!(mem.read32(fifo_a), 0x8888_8888, "source advanced");
    }

    #[test]
    fn decrementing_addresses() {
        let mut mem = Ram::new();
        mem.write16(0x1000, 1);
        mem.write16(0x1002, 2);
        // src dec (1 << 7), dst dec (1 << 5)
        let mut dma = armed(2, 0x1002, 0x2002, 2, 0x8000 | (1 << 7) | (1 << 5));
        run(&mut dma, 2, &mut mem);
        assert_eq!(mem.read16(0x2002), 2);
        assert_eq!(mem.read16(0x2000), 1);
    }
}
