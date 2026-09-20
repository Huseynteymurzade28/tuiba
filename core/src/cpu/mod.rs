//! ARM7TDMI CPU core.
//!
//! # Pipeline model
//!
//! The ARM7TDMI has a three-stage fetch/decode/execute pipeline, and
//! software can observe it: reading r15 yields the address of the
//! instruction being executed **plus 8** (ARM) or **plus 4** (THUMB). We
//! reproduce that with a two-entry prefetch queue. At any time:
//!
//! - `pipeline[0]` holds the instruction about to execute,
//! - `pipeline[1]` holds the one after it,
//! - r15 is the address the *next* fetch will come from, which is exactly
//!   the value software expects to see.
//!
//! Any write to r15 flushes the queue and refills it from the new address.

pub mod arm;
pub mod registers;
pub mod thumb;

use crate::error::Result;
use crate::memory::Memory;
pub use registers::{Cpsr, Mode, Registers};
use registers::{LR, PC, SP};

/// Exception vectors and the mode each one enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exception {
    /// Power-on / reset.
    Reset,
    /// Undefined instruction.
    Undefined,
    /// `SWI` instruction.
    SoftwareInterrupt,
    /// Instruction fetch abort (never raised on GBA).
    PrefetchAbort,
    /// Data access abort (never raised on GBA).
    DataAbort,
    /// Normal interrupt request.
    Irq,
    /// Fast interrupt request (not wired up on GBA).
    Fiq,
}

impl Exception {
    /// Address of the exception handler.
    #[must_use]
    pub const fn vector(self) -> u32 {
        match self {
            Self::Reset => 0x00,
            Self::Undefined => 0x04,
            Self::SoftwareInterrupt => 0x08,
            Self::PrefetchAbort => 0x0C,
            Self::DataAbort => 0x10,
            Self::Irq => 0x18,
            Self::Fiq => 0x1C,
        }
    }

    /// Mode the processor switches to.
    #[must_use]
    pub const fn mode(self) -> Mode {
        match self {
            Self::Reset | Self::SoftwareInterrupt => Mode::Supervisor,
            Self::Undefined => Mode::Undefined,
            Self::PrefetchAbort | Self::DataAbort => Mode::Abort,
            Self::Irq => Mode::Irq,
            Self::Fiq => Mode::Fiq,
        }
    }
}

/// The processor state.
#[derive(Debug, Clone)]
pub struct Cpu {
    /// Register file.
    pub regs: Registers,
    /// Prefetch queue; see the module docs.
    pipeline: [u32; 2],
    /// Set when r15 was written this step, so the pipeline must be refilled
    /// instead of advanced.
    flushed: bool,
    /// Total cycles executed. Purely informational for now.
    pub cycles: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// Creates a CPU in the reset state with an empty pipeline.
    ///
    /// Call [`Cpu::reset`] or [`Cpu::skip_bios`] before stepping.
    #[must_use]
    pub fn new() -> Self {
        Self {
            regs: Registers::new(),
            pipeline: [0; 2],
            flushed: false,
            cycles: 0,
        }
    }

    /// Resets the CPU and starts executing the BIOS at `0x0000_0000`.
    pub fn reset(&mut self, mem: &mut impl Memory) {
        self.regs = Registers::new();
        self.cycles = 0;
        self.flush_pipeline(mem, 0);
    }

    /// Puts the CPU in the state the BIOS leaves it in and jumps to the
    /// cartridge entry point, for running without a BIOS image.
    pub fn skip_bios(&mut self, mem: &mut impl Memory) {
        self.regs = Registers::new();
        self.cycles = 0;
        // Stacks the BIOS sets up for each mode.
        self.regs.switch_mode(Mode::Irq);
        self.regs.set(SP, 0x0300_7FA0);
        self.regs.switch_mode(Mode::Supervisor);
        self.regs.set(SP, 0x0300_7FE0);
        self.regs.switch_mode(Mode::System);
        self.regs.set(SP, 0x0300_7F00);
        self.regs.cpsr.set_irq_disabled(false);
        self.regs.cpsr.set_fiq_disabled(false);
        self.flush_pipeline(mem, 0x0800_0000);
    }

    /// Whether the CPU is in THUMB state.
    #[must_use]
    pub fn thumb(&self) -> bool {
        self.regs.cpsr.thumb()
    }

    /// Instruction size in bytes for the current state.
    #[inline]
    fn instruction_size(&self) -> u32 {
        if self.thumb() { 2 } else { 4 }
    }

    /// Address of the instruction that would execute on the next step.
    #[must_use]
    pub fn next_pc(&self) -> u32 {
        self.regs.get(PC).wrapping_sub(2 * self.instruction_size())
    }

    #[inline]
    fn fetch(&self, mem: &impl Memory, address: u32) -> u32 {
        if self.thumb() {
            u32::from(mem.read16(address))
        } else {
            mem.read32(address)
        }
    }

    /// Refills the prefetch queue from `address` (aligned for the current
    /// state) and points r15 past it.
    fn flush_pipeline(&mut self, mem: &impl Memory, address: u32) {
        let size = self.instruction_size();
        let address = address & !(size - 1);
        self.pipeline[0] = self.fetch(mem, address);
        self.pipeline[1] = self.fetch(mem, address.wrapping_add(size));
        self.regs.set(PC, address.wrapping_add(2 * size));
        self.flushed = false;
    }

    /// Writes r15 and marks the pipeline for refill at the end of the step.
    ///
    /// The value is aligned to the instruction size of the *current* state,
    /// so callers switching state (`BX`) must update the T bit first.
    pub(crate) fn set_pc(&mut self, address: u32) {
        let size = self.instruction_size();
        self.regs.set(PC, address & !(size - 1));
        self.flushed = true;
    }

    /// Takes `exception`: saves CPSR to the new mode's SPSR, banks
    /// registers, sets LR to the return address and jumps to the vector.
    ///
    /// Like any other write to r15 this only *schedules* the pipeline
    /// refill; it happens at the end of the current step.
    pub(crate) fn enter_exception(&mut self, exception: Exception) {
        // LR = address of the *next* instruction for SWI/UND (return with
        // `MOVS pc, lr`), and next+4 for IRQ (return with `SUBS pc, lr, #4`).
        // In both cases that is r15 minus one instruction.
        let return_address = self.regs.get(PC).wrapping_sub(self.instruction_size());
        let old_cpsr = self.regs.cpsr;

        self.regs.switch_mode(exception.mode());
        self.regs.set_spsr(old_cpsr);
        self.regs.set(LR, return_address);
        self.regs.cpsr.set_thumb(false);
        self.regs.cpsr.set_irq_disabled(true);
        if matches!(exception, Exception::Reset | Exception::Fiq) {
            self.regs.cpsr.set_fiq_disabled(true);
        }
        self.set_pc(exception.vector());
    }

    /// Executes a single instruction and returns the cycles it took.
    ///
    /// # Errors
    ///
    /// Returns [`crate::GbaError::UnimplementedInstruction`] for opcodes the
    /// core cannot execute yet. The CPU state is left as it was before the
    /// instruction, so the caller can inspect it.
    pub fn step(&mut self, mem: &mut impl Memory) -> Result<u32> {
        let size = self.instruction_size();
        let pc = self.regs.get(PC);
        let address = pc.wrapping_sub(2 * size);

        let op = self.pipeline[0];
        self.pipeline[0] = self.pipeline[1];
        self.pipeline[1] = self.fetch(mem, pc);

        let cycles = if self.thumb() {
            self.execute_thumb(op as u16, address)
        } else {
            self.execute_arm(op, address)
        };

        match cycles {
            Ok(cycles) => {
                if self.flushed {
                    self.flush_pipeline(mem, self.regs.get(PC));
                } else {
                    self.regs.set(PC, pc.wrapping_add(size));
                }
                self.cycles += u64::from(cycles);
                Ok(cycles)
            }
            Err(err) => {
                // Undo the prefetch so a retry (or a debugger) sees the
                // faulting instruction at the front of the queue.
                self.pipeline[1] = self.pipeline[0];
                self.pipeline[0] = op;
                Err(err)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use crate::memory::Memory;

    /// Flat 64 KiB RAM for CPU tests; addresses wrap.
    pub struct Ram(pub Vec<u8>);

    impl Ram {
        pub fn new() -> Self {
            Self(vec![0; 0x1_0000])
        }

        /// Assembles pre-encoded ARM words at `base`.
        pub fn load_arm(&mut self, base: u32, words: &[u32]) {
            for (i, w) in words.iter().enumerate() {
                self.write32(base + 4 * i as u32, *w);
            }
        }

        /// Assembles pre-encoded THUMB halfwords at `base`.
        pub fn load_thumb(&mut self, base: u32, halves: &[u16]) {
            for (i, h) in halves.iter().enumerate() {
                self.write16(base + 2 * i as u32, *h);
            }
        }

        fn idx(&self, address: u32) -> usize {
            address as usize % self.0.len()
        }
    }

    impl Memory for Ram {
        fn read8(&self, a: u32) -> u8 {
            self.0[self.idx(a)]
        }
        fn read16(&self, a: u32) -> u16 {
            u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
        }
        fn read32(&self, a: u32) -> u32 {
            u32::from(self.read16(a)) | (u32::from(self.read16(a + 2)) << 16)
        }
        fn write8(&mut self, a: u32, v: u8) {
            let i = self.idx(a);
            self.0[i] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.write8(a, v as u8);
            self.write8(a + 1, (v >> 8) as u8);
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.write16(a, v as u16);
            self.write16(a + 2, (v >> 16) as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::Ram;
    use super::*;
    use crate::error::GbaError;

    fn cpu_at(mem: &mut Ram, pc: u32) -> Cpu {
        let mut cpu = Cpu::new();
        cpu.reset(mem);
        cpu.regs.switch_mode(Mode::System);
        cpu.flush_pipeline(mem, pc);
        cpu
    }

    #[test]
    fn reset_starts_at_vector_zero_with_prefetch() {
        let mut mem = Ram::new();
        mem.load_arm(0, &[0xEA00_0000, 0xEA00_0001]);
        let mut cpu = Cpu::new();
        cpu.reset(&mut mem);
        assert_eq!(cpu.regs.get(PC), 8);
        assert_eq!(cpu.next_pc(), 0);
        assert_eq!(cpu.pipeline, [0xEA00_0000, 0xEA00_0001]);
    }

    #[test]
    fn skip_bios_sets_up_stacks_and_entry_point() {
        let mut mem = Ram::new();
        let mut cpu = Cpu::new();
        cpu.skip_bios(&mut mem);
        assert_eq!(cpu.regs.mode(), Mode::System);
        assert_eq!(cpu.regs.get(SP), 0x0300_7F00);
        assert_eq!(cpu.next_pc(), 0x0800_0000);
        assert!(!cpu.regs.cpsr.irq_disabled());
        cpu.regs.switch_mode(Mode::Irq);
        assert_eq!(cpu.regs.get(SP), 0x0300_7FA0);
        cpu.regs.switch_mode(Mode::Supervisor);
        assert_eq!(cpu.regs.get(SP), 0x0300_7FE0);
    }

    #[test]
    fn arm_branch_forward_and_backward() {
        let mut mem = Ram::new();
        // 0x100: b 0x110 ; offset = (0x110 - 0x108) / 4 = 2
        mem.load_arm(0x100, &[0xEA00_0002]);
        // 0x110: b 0x100 ; offset = (0x100 - 0x118) / 4 = -6
        mem.load_arm(0x110, &[0xEAFF_FFFA]);
        let mut cpu = cpu_at(&mut mem, 0x100);
        assert_eq!(cpu.step(&mut mem).unwrap(), 3);
        assert_eq!(cpu.next_pc(), 0x110);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x100);
    }

    #[test]
    fn arm_branch_with_link_sets_return_address() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xEB00_0010]); // bl 0x148
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(LR), 0x104);
        assert_eq!(cpu.next_pc(), 0x148);
    }

    #[test]
    fn failed_condition_skips_instruction() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0x0A00_0010, 0xEA00_0000]); // beq +; b +0
        let mut cpu = cpu_at(&mut mem, 0x100);
        assert_eq!(cpu.step(&mut mem).unwrap(), 1);
        assert_eq!(cpu.next_pc(), 0x104);
        cpu.regs.cpsr.set_z(true);
        cpu.flush_pipeline(&mem, 0x100);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x148);
    }

    #[test]
    fn bx_switches_to_thumb_and_back() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE12F_FF10]); // bx r0
        mem.load_thumb(0x200, &[0x4708]); // bx r1
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x201);
        cpu.regs.set(1, 0x300);
        cpu.step(&mut mem).unwrap();
        assert!(cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x200);
        assert_eq!(cpu.regs.get(PC), 0x204, "THUMB r15 reads PC+4");
        cpu.step(&mut mem).unwrap();
        assert!(!cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x300);
        assert_eq!(cpu.regs.get(PC), 0x308, "ARM r15 reads PC+8");
    }

    #[test]
    fn thumb_branches() {
        let mut mem = Ram::new();
        // 0x200: b +4 (offset 2 halfwords -> target 0x204+4 = 0x208)
        mem.load_thumb(0x200, &[0xE002]);
        // 0x208: bne -6 -> 0x20C - 6 = 0x206 ; then beq
        mem.load_thumb(0x208, &[0xD1FD]);
        let mut cpu = cpu_at(&mut mem, 0x200);
        cpu.regs.cpsr.set_thumb(true);
        cpu.flush_pipeline(&mem, 0x200);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x208);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x206);
        cpu.regs.cpsr.set_z(true);
        cpu.flush_pipeline(&mem, 0x208);
        assert_eq!(cpu.step(&mut mem).unwrap(), 1);
        assert_eq!(cpu.next_pc(), 0x20A);
    }

    #[test]
    fn thumb_long_branch_link() {
        let mut mem = Ram::new();
        // bl 0x400 from 0x200: pc = 0x204, offset = 0x1FC
        // hi = F000 | (0x1FC >> 12) = F000 ; lo = F800 | ((0x1FC >> 1) & 0x7FF) = F8FE
        mem.load_thumb(0x200, &[0xF000, 0xF8FE]);
        let mut cpu = cpu_at(&mut mem, 0x200);
        cpu.regs.cpsr.set_thumb(true);
        cpu.flush_pipeline(&mem, 0x200);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x400);
        assert_eq!(cpu.regs.get(LR), 0x205);
    }

    #[test]
    fn swi_enters_supervisor_mode() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xEF00_0001]); // swi 1
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.regs.cpsr.set_c(true);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.mode(), Mode::Supervisor);
        assert_eq!(cpu.next_pc(), 0x08);
        assert_eq!(cpu.regs.get(LR), 0x104);
        assert!(cpu.regs.cpsr.irq_disabled());
        let spsr = cpu.regs.spsr().unwrap();
        assert!(spsr.c());
        assert_eq!(spsr.mode(), Some(Mode::System));
    }

    #[test]
    fn thumb_swi_returns_to_arm_state_handler() {
        let mut mem = Ram::new();
        mem.load_thumb(0x200, &[0xDF02]);
        let mut cpu = cpu_at(&mut mem, 0x200);
        cpu.regs.cpsr.set_thumb(true);
        cpu.flush_pipeline(&mem, 0x200);
        cpu.step(&mut mem).unwrap();
        assert!(!cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x08);
        assert_eq!(cpu.regs.get(LR), 0x202);
        assert!(cpu.regs.spsr().unwrap().thumb());
    }

    #[test]
    fn undefined_instruction_traps() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE780_1012]);
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.mode(), Mode::Undefined);
        assert_eq!(cpu.next_pc(), 0x04);
    }

    #[test]
    fn unimplemented_reports_address_and_leaves_state() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE3A0_0001]); // mov r0, #1
        let mut cpu = cpu_at(&mut mem, 0x100);
        let err = cpu.step(&mut mem).unwrap_err();
        assert!(matches!(
            err,
            GbaError::UnimplementedInstruction {
                mode: "ARM",
                opcode: 0xE3A0_0001,
                pc: 0x100
            }
        ));
        assert_eq!(cpu.next_pc(), 0x100);
        assert_eq!(cpu.pipeline[0], 0xE3A0_0001);
    }
}
