//! `--profile`: where the CPU spends its cycles during a headless run.
//!
//! Instead of `Gba::run_frame`, profiled frames step the machine one
//! instruction at a time and charge each instruction's cycles to its
//! address, together with the link register at that moment — the
//! return address of the function it belongs to, as long as that
//! function has not called another one yet. Only headless runs that ask
//! for a profile go through here; the interactive loop never does.

use std::collections::HashMap;
use std::io::{self, Write};

use tuiba_core::cpu::disasm;
use tuiba_core::ppu::CYCLES_PER_FRAME;
use tuiba_core::{Gba, Memory};

/// How many addresses the report lists.
const TOP: usize = 20;

/// Cycles charged to one instruction address.
#[derive(Debug, Default)]
struct Entry {
    cycles: u64,
    thumb: bool,
    /// Cycles by link register value.
    callers: HashMap<u32, u64>,
}

/// A PC histogram over the profiled frames.
#[derive(Debug, Default)]
pub struct Profile {
    entries: HashMap<u32, Entry>,
    frames: u32,
    busy: u64,
}

impl Profile {
    /// Runs one frame of `gba`, charging every instruction's cycles.
    pub fn run_frame(&mut self, gba: &mut Gba) {
        loop {
            let running = !gba.cpu.halted;
            let pc = gba.cpu.next_pc();
            let thumb = gba.cpu.thumb();
            let lr = gba.cpu.regs.get(14);
            let before = gba.cpu.cycles;
            let done = gba.step();
            if running {
                self.charge(pc, thumb, lr, gba.cpu.cycles - before);
            }
            if done {
                break;
            }
        }
        self.frames += 1;
    }

    fn charge(&mut self, pc: u32, thumb: bool, lr: u32, cycles: u64) {
        let entry = self.entries.entry(pc).or_default();
        entry.cycles += cycles;
        entry.thumb = thumb;
        // THUMB return addresses carry the state in bit 0.
        *entry.callers.entry(lr & !1).or_default() += cycles;
        self.busy += cycles;
    }

    /// Writes the report: the share of time the CPU ran, then the
    /// busiest addresses with their instruction and most frequent
    /// return address. `memory` supplies the instructions' text.
    pub fn write(&self, mut out: impl Write, memory: &impl Memory) -> io::Result<()> {
        let total = u64::from(self.frames) * u64::from(CYCLES_PER_FRAME);
        #[allow(clippy::cast_precision_loss)] // a percentage needs no 53rd bit
        let share = |cycles: u64| 100.0 * cycles as f64 / total.max(1) as f64;
        writeln!(
            out,
            "profile: {} frames, {total} cycles; CPU running {:.1} %, halted or held by DMA {:.1} %",
            self.frames,
            share(self.busy),
            share(total.saturating_sub(self.busy)),
        )?;
        writeln!(
            out,
            "  {:>10}  {:>6}  {:8}  {:34}  return address",
            "cycles", "share", "address", "instruction"
        )?;
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_by(|a, b| b.1.cycles.cmp(&a.1.cycles).then(a.0.cmp(b.0)));
        for (&pc, entry) in entries.into_iter().take(TOP) {
            let text = if entry.thumb {
                let next = memory.read16(pc.wrapping_add(2));
                disasm::thumb(pc, memory.read16(pc), next).0
            } else {
                disasm::arm(pc, memory.read32(pc))
            };
            // Most cycles first; ties to the lower address, for stable output.
            let (lr, lr_cycles) = entry
                .callers
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map_or((0, 0), |(&lr, &c)| (lr, c));
            let lr_share = 100 * lr_cycles / entry.cycles.max(1);
            writeln!(
                out,
                "  {:>10}  {:>5.1}%  {pc:08x}  {text:34}  {lr:08x} ({lr_share} %)",
                entry.cycles,
                share(entry.cycles),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuiba_core::Cartridge;

    /// A ROM whose entry point spins on `b .` in ARM state.
    fn spinning() -> Gba {
        let mut rom = vec![0u8; 0x200];
        rom[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
        Gba::new(Cartridge::from_bytes(rom).expect("valid ROM"))
    }

    #[test]
    fn a_spin_loop_takes_every_cycle() {
        let mut gba = spinning();
        let mut profile = Profile::default();
        gba.run_frame(); // past the boot state
        profile.run_frame(&mut gba);
        profile.run_frame(&mut gba);

        assert_eq!(profile.frames, 2);
        assert_eq!(profile.entries.len(), 1);
        let entry = &profile.entries[&0x0800_0000];
        assert!(!entry.thumb);
        assert_eq!(entry.cycles, profile.busy);
        // A frame ends after the instruction that crosses into VBlank,
        // so each one may run over or under by an instruction.
        let total = 2 * u64::from(CYCLES_PER_FRAME);
        assert!(
            total.abs_diff(profile.busy) < 2 * 20,
            "busy {}",
            profile.busy
        );

        let mut out = Vec::new();
        profile.write(&mut out, &gba.bus).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[0].starts_with("profile: 2 frames, 561792 cycles; CPU running 100.0 %"));
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[2].contains("08000000  b 0x08000000"), "{text}");
    }

    #[test]
    fn callers_are_ranked_by_cycles() {
        let mut profile = Profile {
            frames: 1,
            ..Profile::default()
        };
        profile.charge(0x0800_0100, true, 0x0800_0201, 10);
        profile.charge(0x0800_0100, true, 0x0800_0301, 30);
        profile.charge(0x0800_0104, true, 0x0800_0201, 5);
        let entry = &profile.entries[&0x0800_0100];
        assert_eq!(entry.cycles, 40);
        assert_eq!(entry.callers[&0x0800_0300], 30, "bit 0 is dropped");

        let mut out = Vec::new();
        profile.write(&mut out, &spinning().bus).unwrap();
        let text = String::from_utf8(out).unwrap();
        let rows: Vec<_> = text.lines().skip(2).collect();
        assert!(rows[0].contains("08000100"), "{text}");
        assert!(rows[0].ends_with("08000300 (75 %)"), "{text}");
        assert!(rows[1].contains("08000104"), "{text}");
    }
}
