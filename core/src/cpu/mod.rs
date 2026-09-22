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

pub mod alu;
pub mod arm;
pub mod load;
pub mod registers;
pub mod thumb;

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// When set, `SWI` does not enter the exception vector but records the
    /// call number for the emulator to service in software (HLE BIOS).
    pub hle_swi: bool,
    /// Pending HLE `SWI` call number, taken by [`Cpu::take_swi`].
    pending_swi: Option<u8>,
    /// Halted by `HALTCNT` or a BIOS wait call; woken by an interrupt.
    pub halted: bool,
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
            hle_swi: false,
            pending_swi: None,
            halted: false,
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
    pub(crate) fn flush_pipeline(&mut self, mem: &impl Memory, address: u32) {
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
        // SWI/UND run mid-step: LR = the next instruction, i.e. r15 minus
        // one instruction (return with `MOVS pc, lr`).
        // IRQ/FIQ are taken between steps: LR = next instruction + 4
        // (return with `SUBS pc, lr, #4`). With r15 = next + 2*size that is
        // r15 - 4 in ARM and exactly r15 in THUMB.
        let pc = self.regs.get(PC);
        let return_address = match exception {
            Exception::Irq | Exception::Fiq => {
                if self.thumb() {
                    pc
                } else {
                    pc.wrapping_sub(4)
                }
            }
            _ => pc.wrapping_sub(self.instruction_size()),
        };
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

    /// Handles an `SWI` instruction: either records it for HLE servicing
    /// or enters the Supervisor exception vector.
    pub(crate) fn software_interrupt(&mut self, comment: u8) {
        if self.hle_swi {
            self.pending_swi = Some(comment);
        } else {
            self.enter_exception(Exception::SoftwareInterrupt);
        }
    }

    /// Takes the HLE `SWI` call recorded by the last step, if any.
    pub fn take_swi(&mut self) -> Option<u8> {
        self.pending_swi.take()
    }

    /// Whether an IRQ would be accepted right now (CPSR I bit clear).
    #[must_use]
    pub fn irq_enabled(&self) -> bool {
        !self.regs.cpsr.irq_disabled()
    }

    /// Takes the IRQ exception immediately, refilling the pipeline from
    /// the vector. Call between steps, only when [`Cpu::irq_enabled`].
    pub fn raise_irq(&mut self, mem: &impl Memory) {
        self.halted = false;
        self.enter_exception(Exception::Irq);
        self.flush_pipeline(mem, self.regs.get(PC));
    }

    /// Executes a single instruction and returns the cycles it took:
    /// the bus cycles of its fetch and data accesses (including any
    /// pipeline refill) plus the instruction's internal cycles.
    pub fn step(&mut self, mem: &mut impl Memory) -> u32 {
        let size = self.instruction_size();
        let pc = self.regs.get(PC);

        let op = self.pipeline[0];
        self.pipeline[0] = self.pipeline[1];
        self.pipeline[1] = self.fetch(mem, pc);

        let internal = if self.thumb() {
            self.execute_thumb(mem, op as u16)
        } else {
            self.execute_arm(mem, op)
        };

        if self.flushed {
            self.flush_pipeline(mem, self.regs.get(PC));
        } else {
            self.regs.set(PC, pc.wrapping_add(size));
        }
        mem.idle(internal);
        let cycles = internal + mem.take_access_cycles();
        self.cycles += u64::from(cycles);
        cycles
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::cell::Cell;

    use super::{Cpu, Mode};
    use crate::memory::Memory;

    /// A CPU in System mode, ARM state, about to execute the word at `pc`.
    pub fn arm_at(mem: &mut Ram, pc: u32) -> Cpu {
        let mut cpu = Cpu::new();
        cpu.reset(mem);
        cpu.regs.switch_mode(Mode::System);
        cpu.flush_pipeline(mem, pc);
        // Setup accesses must not count against the first instruction.
        mem.take_access_cycles();
        cpu
    }

    /// A CPU in System mode, THUMB state, about to execute the halfword at `pc`.
    pub fn thumb_at(mem: &mut Ram, pc: u32) -> Cpu {
        let mut cpu = arm_at(mem, pc);
        cpu.regs.cpsr.set_thumb(true);
        cpu.flush_pipeline(mem, pc);
        mem.take_access_cycles();
        cpu
    }

    /// Hand encoders for instructions whose bit layout is easy to get wrong.
    pub mod enc {
        /// ARM data processing with a rotated 8-bit immediate (`AL` condition).
        pub fn dp_imm(opcode: u32, s: bool, rn: u32, rd: u32, imm8: u32, rot4: u32) -> u32 {
            0xE200_0000
                | (opcode << 21)
                | (u32::from(s) << 20)
                | (rn << 16)
                | (rd << 12)
                | (rot4 << 8)
                | imm8
        }
    }

    /// Flat 64 KiB RAM for CPU tests; addresses wrap.
    pub struct Ram {
        pub bytes: Vec<u8>,
        accesses: Cell<u32>,
    }

    impl Ram {
        pub fn new() -> Self {
            Self {
                bytes: vec![0; 0x1_0000],
                accesses: Cell::new(0),
            }
        }

        fn byte(&self, address: u32) -> u8 {
            self.bytes[self.idx(address)]
        }

        fn set_byte(&mut self, address: u32, value: u8) {
            let i = self.idx(address);
            self.bytes[i] = value;
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
            address as usize % self.bytes.len()
        }
    }

    /// Every access costs one cycle, so instruction timings in tests
    /// read as plain S/N/I counts.
    impl Memory for Ram {
        fn read8(&self, a: u32) -> u8 {
            self.accesses.set(self.accesses.get() + 1);
            self.byte(a)
        }
        fn read16(&self, a: u32) -> u16 {
            self.accesses.set(self.accesses.get() + 1);
            u16::from_le_bytes([self.byte(a), self.byte(a + 1)])
        }
        fn read32(&self, a: u32) -> u32 {
            self.accesses.set(self.accesses.get() + 1);
            u32::from_le_bytes([
                self.byte(a),
                self.byte(a + 1),
                self.byte(a + 2),
                self.byte(a + 3),
            ])
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.accesses.set(self.accesses.get() + 1);
            self.set_byte(a, v);
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.accesses.set(self.accesses.get() + 1);
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.set_byte(a + i as u32, b);
            }
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.accesses.set(self.accesses.get() + 1);
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.set_byte(a + i as u32, b);
            }
        }
        fn take_access_cycles(&self) -> u32 {
            self.accesses.replace(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{Ram, arm_at as cpu_at, thumb_at};
    use super::*;

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
        assert_eq!(cpu.step(&mut mem), 3, "2S + 1N");
        assert_eq!(cpu.next_pc(), 0x110);
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x100);
    }

    #[test]
    fn arm_branch_with_link_sets_return_address() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xEB00_0010]); // bl 0x148
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(LR), 0x104);
        assert_eq!(cpu.next_pc(), 0x148);
    }

    #[test]
    fn failed_condition_skips_instruction() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0x0A00_0010, 0xEA00_0000]); // beq +; b +0
        let mut cpu = cpu_at(&mut mem, 0x100);
        assert_eq!(cpu.step(&mut mem), 1);
        assert_eq!(cpu.next_pc(), 0x104);
        cpu.regs.cpsr.set_z(true);
        cpu.flush_pipeline(&mem, 0x100);
        cpu.step(&mut mem);
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
        cpu.step(&mut mem);
        assert!(cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x200);
        assert_eq!(cpu.regs.get(PC), 0x204, "THUMB r15 reads PC+4");
        cpu.step(&mut mem);
        assert!(!cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x300);
        assert_eq!(cpu.regs.get(PC), 0x308, "ARM r15 reads PC+8");
    }

    /// With one cycle per access, the totals are the S/N/I sums from the
    /// ARM7TDMI data sheet.
    #[test]
    fn instruction_timings() {
        let cases: &[(u32, u32, &str)] = &[
            (0xE3A0_0001, 1, "mov r0, #1: 1S"),
            (0xE1A0_0211, 2, "mov r0, r1, lsl r2: 1S + 1I"),
            (0xE591_0000, 3, "ldr r0, [r1]: 1S + 1N + 1I"),
            (0xE581_0000, 2, "str r0, [r1]: 2N"),
            (0xE1D1_00B0, 3, "ldrh r0, [r1]: 1S + 1N + 1I"),
            (0xE891_001E, 6, "ldmia r1, {r1-r4}: 4S + 1N + 1I"),
            (0xE881_001E, 5, "stmia r1, {r1-r4}: 3S + 2N"),
            (0xE001_0290, 2, "mul r1, r0, r2 (small r2): 1S + 1I"),
            (0xE101_0092, 4, "swp r0, r2, [r1]: 1S + 2N + 1I"),
            (0xEA00_0000, 3, "b: 2S + 1N"),
            (0xE591_F000, 5, "ldr pc, [r1]: 1S + 1N + 1I + refill"),
            (0xE1A0_F00E, 3, "mov pc, lr: 2S + 1N"),
        ];
        for &(word, expected, name) in cases {
            let mut mem = Ram::new();
            mem.load_arm(0x100, &[word]);
            let mut cpu = cpu_at(&mut mem, 0x100);
            cpu.regs.set(1, 0x2000);
            cpu.regs.set(2, 3);
            cpu.regs.set(LR, 0x300);
            assert_eq!(cpu.step(&mut mem), expected, "{name}");
        }

        // A large multiplier costs more internal cycles.
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE001_0290]);
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.regs.set(2, 0x1234_5678);
        assert_eq!(
            cpu.step(&mut mem),
            5,
            "mul with a 4-byte multiplier: 1S + 4I"
        );

        let thumb: &[(u16, u32, &str)] = &[
            (0x2001, 1, "movs r0, #1: 1S"),
            (0x4090, 2, "lsl r0, r2: 1S + 1I"),
            (0x6808, 3, "ldr r0, [r1]: 1S + 1N + 1I"),
            (0x6008, 2, "str r0, [r1]: 2N"),
            (0xBC0E, 5, "pop {r1-r3}: 3S + 1N + 1I"),
            (0xB40E, 4, "push {r1-r3}: 2S + 2N"),
            (0xE000, 3, "b: 2S + 1N"),
        ];
        for &(half, expected, name) in thumb {
            let mut mem = Ram::new();
            mem.load_thumb(0x200, &[half]);
            let mut cpu = thumb_at(&mut mem, 0x200);
            cpu.regs.set(1, 0x2000);
            cpu.regs.set(2, 3);
            cpu.regs.set(SP, 0x3000);
            assert_eq!(cpu.step(&mut mem), expected, "{name}");
        }
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
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x208);
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x206);
        cpu.regs.cpsr.set_z(true);
        cpu.flush_pipeline(&mem, 0x208);
        mem.take_access_cycles();
        assert_eq!(cpu.step(&mut mem), 1, "failed condition: 1S");
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
        cpu.step(&mut mem);
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x400);
        assert_eq!(cpu.regs.get(LR), 0x205);
    }

    #[test]
    fn swi_enters_supervisor_mode() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xEF00_0001]); // swi 1
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.regs.cpsr.set_c(true);
        cpu.step(&mut mem);
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
        cpu.step(&mut mem);
        assert!(!cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x08);
        assert_eq!(cpu.regs.get(LR), 0x202);
        assert!(cpu.regs.spsr().unwrap().thumb());
    }

    #[test]
    fn irq_return_address_is_next_plus_four_in_both_states() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE1A0_0000; 4]);
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.step(&mut mem);
        cpu.raise_irq(&mem);
        assert_eq!(cpu.regs.mode(), Mode::Irq);
        assert_eq!(cpu.regs.get(LR), 0x108, "next instruction 0x104 + 4");
        assert_eq!(cpu.next_pc(), 0x18);
        assert!(cpu.regs.cpsr.irq_disabled());

        mem.load_thumb(0x200, &[0x46C0; 4]); // nop
        let mut cpu = cpu_at(&mut mem, 0x200);
        cpu.regs.cpsr.set_thumb(true);
        cpu.flush_pipeline(&mem, 0x200);
        cpu.step(&mut mem);
        cpu.raise_irq(&mem);
        assert!(!cpu.thumb());
        assert_eq!(cpu.regs.get(LR), 0x206, "next instruction 0x202 + 4");
        assert!(cpu.regs.spsr().unwrap().thumb());
    }

    #[test]
    fn hle_swi_is_recorded_instead_of_taken() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xEF05_0000]); // swi 0x50000: ARM form of call 5
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.hle_swi = true;
        cpu.step(&mut mem);
        assert_eq!(cpu.take_swi(), Some(5));
        assert_eq!(cpu.take_swi(), None);
        assert_eq!(cpu.regs.mode(), Mode::System);
        assert_eq!(cpu.next_pc(), 0x104);
    }

    #[test]
    fn undefined_instruction_traps() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE780_1012]);
        let mut cpu = cpu_at(&mut mem, 0x100);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.mode(), Mode::Undefined);
        assert_eq!(cpu.next_pc(), 0x04);
    }
}
