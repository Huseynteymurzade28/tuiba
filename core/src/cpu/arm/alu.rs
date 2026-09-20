//! ARM data processing, PSR transfer and multiply instructions.

use crate::cpu::Cpu;
use crate::cpu::alu::{ShiftType, add_with_carry, shift_imm, shift_reg};
use crate::cpu::registers::{Cpsr, PC};

/// Data-processing opcodes (bits 24:21).
mod opcode {
    pub const AND: u32 = 0x0;
    pub const EOR: u32 = 0x1;
    pub const SUB: u32 = 0x2;
    pub const RSB: u32 = 0x3;
    pub const ADD: u32 = 0x4;
    pub const ADC: u32 = 0x5;
    pub const SBC: u32 = 0x6;
    pub const RSC: u32 = 0x7;
    pub const TST: u32 = 0x8;
    pub const TEQ: u32 = 0x9;
    pub const CMP: u32 = 0xA;
    pub const CMN: u32 = 0xB;
    pub const ORR: u32 = 0xC;
    pub const MOV: u32 = 0xD;
    pub const BIC: u32 = 0xE;
    // 0xF is MVN, handled by the fallthrough arm.
}

/// Rotates an 8-bit immediate (bits 7:0) right by twice the 4-bit field
/// in bits 11:8. Used by data processing and `MSR`.
#[inline]
const fn rotated_immediate(op: u32) -> (u32, u32) {
    let rot = ((op >> 8) & 0xF) * 2;
    ((op & 0xFF).rotate_right(rot), rot)
}

impl Cpu {
    /// Reads `Rn`/`Rm` for the register-specified-shift form, where r15
    /// reads as PC+12 instead of PC+8.
    #[inline]
    fn reg_plus4_if_pc(&self, index: usize) -> u32 {
        let value = self.regs.get(index);
        if index == PC {
            value.wrapping_add(4)
        } else {
            value
        }
    }

    /// Evaluates operand 2 of a data-processing instruction.
    ///
    /// Returns `(value, shifter_carry, register_shift)`; the last flag
    /// tells the caller that r15 reads +4 and one extra cycle is due.
    fn arm_operand2(&self, op: u32) -> (u32, bool, bool) {
        let carry_in = self.regs.cpsr.c();
        if op & (1 << 25) != 0 {
            let (value, rot) = rotated_immediate(op);
            let carry = if rot == 0 { carry_in } else { value >> 31 != 0 };
            return (value, carry, false);
        }

        let rm = (op & 0xF) as usize;
        let kind = ShiftType::from_bits(op >> 5);
        if op & (1 << 4) != 0 {
            let rs = ((op >> 8) & 0xF) as usize;
            let amount = self.regs.get(rs) & 0xFF;
            let (value, carry) = shift_reg(kind, self.reg_plus4_if_pc(rm), amount, carry_in);
            (value, carry, true)
        } else {
            let amount = (op >> 7) & 0x1F;
            let (value, carry) = shift_imm(kind, self.regs.get(rm), amount, carry_in);
            (value, carry, false)
        }
    }

    /// Executes a data-processing instruction.
    pub(super) fn arm_data_processing(&mut self, op: u32) -> u32 {
        let opcode = (op >> 21) & 0xF;
        let set_flags = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;

        let (op2, shifter_carry, reg_shift) = self.arm_operand2(op);
        let rn_val = if reg_shift {
            self.reg_plus4_if_pc(rn)
        } else {
            self.regs.get(rn)
        };
        let carry = self.regs.cpsr.c();
        let cycles = if reg_shift { 2 } else { 1 };

        // `arith` carries the (C, V) produced by an add/sub; logical ops
        // use the shifter carry and leave V alone.
        let (result, arith) = match opcode {
            opcode::AND | opcode::TST => (rn_val & op2, None),
            opcode::EOR | opcode::TEQ => (rn_val ^ op2, None),
            opcode::SUB | opcode::CMP => Self::arith(rn_val, !op2, true),
            opcode::RSB => Self::arith(op2, !rn_val, true),
            opcode::ADD | opcode::CMN => Self::arith(rn_val, op2, false),
            opcode::ADC => Self::arith(rn_val, op2, carry),
            opcode::SBC => Self::arith(rn_val, !op2, carry),
            opcode::RSC => Self::arith(op2, !rn_val, carry),
            opcode::ORR => (rn_val | op2, None),
            opcode::MOV => (op2, None),
            opcode::BIC => (rn_val & !op2, None),
            _ => (!op2, None), // MVN
        };

        let is_test = matches!(
            opcode,
            opcode::TST | opcode::TEQ | opcode::CMP | opcode::CMN
        );

        if rd == PC && !is_test {
            if set_flags {
                // `MOVS pc, lr` and friends: exception return.
                if let Some(spsr) = self.regs.spsr() {
                    self.regs.set_cpsr(spsr);
                }
            }
            self.set_pc(result);
            return cycles + 2;
        }

        if set_flags {
            self.regs.cpsr.set_nz(result);
            match arith {
                Some((c, v)) => {
                    self.regs.cpsr.set_c(c);
                    self.regs.cpsr.set_v(v);
                }
                None => self.regs.cpsr.set_c(shifter_carry),
            }
        }
        if !is_test {
            self.regs.set(rd, result);
        }
        cycles
    }

    #[inline]
    const fn arith(a: u32, b: u32, carry: bool) -> (u32, Option<(bool, bool)>) {
        let (result, c, v) = add_with_carry(a, b, carry);
        (result, Some((c, v)))
    }

    /// Executes `MRS` or `MSR`.
    pub(super) fn arm_psr_transfer(&mut self, op: u32) -> u32 {
        let use_spsr = op & (1 << 22) != 0;
        if op & (1 << 21) == 0 {
            // MRS Rd, CPSR|SPSR
            let rd = ((op >> 12) & 0xF) as usize;
            let value = if use_spsr {
                self.regs.spsr().unwrap_or(self.regs.cpsr)
            } else {
                self.regs.cpsr
            };
            self.regs.set(rd, value.0);
            return 1;
        }

        // MSR CPSR|SPSR_<fields>, Rm|#imm
        let operand = if op & (1 << 25) != 0 {
            rotated_immediate(op).0
        } else {
            self.regs.get((op & 0xF) as usize)
        };
        let mut mask = 0u32;
        for (bit, field) in [
            (16, 0x0000_00FF),
            (17, 0x0000_FF00),
            (18, 0x00FF_0000),
            (19, 0xFF00_0000),
        ] {
            if op & (1 << bit) != 0 {
                mask |= field;
            }
        }
        // User mode may only touch the flag byte.
        if self.regs.mode() == crate::cpu::Mode::User {
            mask &= 0xFF00_0000;
        }

        if use_spsr {
            if let Some(spsr) = self.regs.spsr() {
                self.regs
                    .set_spsr(Cpsr((spsr.0 & !mask) | (operand & mask)));
            }
        } else {
            // Changing T via MSR is unpredictable on hardware; keep it fixed.
            let mask = mask & !(1 << 5);
            let new = Cpsr((self.regs.cpsr.0 & !mask) | (operand & mask));
            self.regs.set_cpsr(new);
        }
        1
    }

    /// Executes `MUL`/`MLA`.
    pub(super) fn arm_multiply(&mut self, op: u32) -> u32 {
        let accumulate = op & (1 << 21) != 0;
        let set_flags = op & (1 << 20) != 0;
        let rd = ((op >> 16) & 0xF) as usize;
        let rn = ((op >> 12) & 0xF) as usize;
        let rs = ((op >> 8) & 0xF) as usize;
        let rm = (op & 0xF) as usize;

        let rs_val = self.regs.get(rs);
        let mut result = self.regs.get(rm).wrapping_mul(rs_val);
        let mut cycles = 1 + multiply_cycles(rs_val, true);
        if accumulate {
            result = result.wrapping_add(self.regs.get(rn));
            cycles += 1;
        }
        self.regs.set(rd, result);
        if set_flags {
            self.regs.cpsr.set_nz(result);
        }
        cycles
    }

    /// Executes `UMULL`/`UMLAL`/`SMULL`/`SMLAL`.
    pub(super) fn arm_multiply_long(&mut self, op: u32) -> u32 {
        let signed = op & (1 << 22) != 0;
        let accumulate = op & (1 << 21) != 0;
        let set_flags = op & (1 << 20) != 0;
        let rd_hi = ((op >> 16) & 0xF) as usize;
        let rd_lo = ((op >> 12) & 0xF) as usize;
        let rs = ((op >> 8) & 0xF) as usize;
        let rm = (op & 0xF) as usize;

        let multiplier = self.regs.get(rs);
        let multiplicand = self.regs.get(rm);
        let mut result: u64 = if signed {
            (i64::from(multiplicand as i32) * i64::from(multiplier as i32)) as u64
        } else {
            u64::from(multiplicand) * u64::from(multiplier)
        };
        let mut cycles = 2 + multiply_cycles(multiplier, signed);
        if accumulate {
            let acc = (u64::from(self.regs.get(rd_hi)) << 32) | u64::from(self.regs.get(rd_lo));
            result = result.wrapping_add(acc);
            cycles += 1;
        }
        self.regs.set(rd_lo, result as u32);
        self.regs.set(rd_hi, (result >> 32) as u32);
        if set_flags {
            self.regs.cpsr.set_n(result >> 63 != 0);
            self.regs.cpsr.set_z(result == 0);
        }
        cycles
    }
}

/// Internal cycles of the multiplier: 1–4 depending on how many
/// significant bytes the multiplier operand has. For signed multiplies,
/// leading all-ones bytes are just as cheap as leading zeros.
const fn multiply_cycles(rs: u32, signed: bool) -> u32 {
    let mut cycles = 1;
    let mut shift = 8;
    while shift < 32 {
        let top = rs >> shift;
        if top == 0 || (signed && top == u32::MAX >> shift) {
            return cycles;
        }
        cycles += 1;
        shift += 8;
    }
    4
}

#[cfg(test)]
mod tests {
    use crate::cpu::registers::LR;
    use crate::cpu::test_util::{Ram, arm_at, enc};
    use crate::cpu::{Cpsr, Mode};

    #[test]
    fn mov_and_add_immediate() {
        let mut mem = Ram::new();
        // mov r0, #1 ; add r1, r0, #0xFF000000 (0xFF ror 8) ; add r2, r1, r0
        mem.load_arm(
            0x100,
            &[
                enc::dp_imm(0xD, false, 0, 0, 1, 0),
                enc::dp_imm(0x4, false, 0, 1, 0xFF, 4),
                0xE081_2000,
            ],
        );
        let mut cpu = arm_at(&mut mem, 0x100);
        for _ in 0..3 {
            cpu.step(&mut mem).unwrap();
        }
        assert_eq!(cpu.regs.get(0), 1);
        assert_eq!(cpu.regs.get(1), 0xFF00_0001);
        assert_eq!(cpu.regs.get(2), 0xFF00_0002);
    }

    #[test]
    fn subs_sets_flags() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[enc::dp_imm(0x2, true, 0, 0, 1, 0)]); // subs r0, r0, #1
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 1);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), 0);
        assert!(cpu.regs.cpsr.z() && cpu.regs.cpsr.c() && !cpu.regs.cpsr.n() && !cpu.regs.cpsr.v());

        cpu.flush_pipeline(&mem, 0x100);
        cpu.step(&mut mem).unwrap(); // 0 - 1
        assert_eq!(cpu.regs.get(0), 0xFFFF_FFFF);
        assert!(!cpu.regs.cpsr.z() && !cpu.regs.cpsr.c() && cpu.regs.cpsr.n());
    }

    #[test]
    fn adds_overflow() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE090_0001]); // adds r0, r0, r1
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x7FFF_FFFF);
        cpu.regs.set(1, 1);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), 0x8000_0000);
        assert!(cpu.regs.cpsr.v() && cpu.regs.cpsr.n() && !cpu.regs.cpsr.c());
    }

    #[test]
    fn cmp_does_not_write_rd() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE150_0001]); // cmp r0, r1
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 5);
        cpu.regs.set(1, 5);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), 5);
        assert!(cpu.regs.cpsr.z() && cpu.regs.cpsr.c());
    }

    #[test]
    fn logical_ops_use_shifter_carry() {
        let mut mem = Ram::new();
        // movs r0, r1, lsl #1 ; ands r2, r0, r1, lsr #4 ; tst r0, #0
        mem.load_arm(0x100, &[0xE1B0_0081, 0xE010_2221, 0xE310_0000]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(1, 0x8000_0018);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), 0x30);
        assert!(cpu.regs.cpsr.c(), "carry out of LSL");
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(2), 0x30 & 0x0800_0001);
        assert!(cpu.regs.cpsr.c(), "carry out of LSR #4 of ...0x18 is bit 3");
        cpu.step(&mut mem).unwrap();
        assert!(cpu.regs.cpsr.z());
        assert!(cpu.regs.cpsr.c(), "rotate 0 keeps carry");
    }

    #[test]
    fn register_shift_reads_pc_plus_12() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE1A0_000F, 0xE1A0_121F]); // mov r0, pc ; mov r1, pc, lsl r2
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(2, 0);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), 0x108);
        assert_eq!(cpu.regs.get(1), 0x110);
    }

    #[test]
    fn writing_pc_flushes_pipeline() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE1A0_F000]); // mov pc, r0
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x203); // misaligned bits dropped
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.next_pc(), 0x200);
    }

    #[test]
    fn movs_pc_lr_restores_spsr() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE1B0_F00E]); // movs pc, lr
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.switch_mode(Mode::Supervisor);
        cpu.regs.set(LR, 0x300);
        cpu.regs.set_spsr(Cpsr(0x2000_0010)); // C set, User mode
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.mode(), Mode::User);
        assert!(cpu.regs.cpsr.c());
        assert_eq!(cpu.next_pc(), 0x300);
    }

    #[test]
    fn mrs_msr_round_trip() {
        let mut mem = Ram::new();
        // mrs r0, cpsr ; orr r0, r0, #0x80000000 ; msr cpsr_f, r0 ; msr cpsr_c, #0x12
        mem.load_arm(0x100, &[0xE10F_0000, 0xE380_0102, 0xE128_F000, 0xE321_F012]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.get(0), cpu.regs.cpsr.0);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert!(cpu.regs.cpsr.n());
        assert_eq!(cpu.regs.mode(), Mode::System);
        cpu.step(&mut mem).unwrap();
        assert_eq!(
            cpu.regs.mode(),
            Mode::Irq,
            "msr control field switches mode"
        );
        assert!(cpu.regs.cpsr.n(), "flag field untouched");
    }

    #[test]
    fn user_mode_msr_cannot_change_mode() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE321_F013]); // msr cpsr_c, #0x13
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.switch_mode(Mode::User);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.mode(), Mode::User);
    }

    #[test]
    fn multiply_variants() {
        let mut mem = Ram::new();
        // mul r0, r1, r2 ; mla r3, r1, r2, r0 ; umull r4, r5, r1, r2 ; smull r6, r7, r1, r2
        mem.load_arm(0x100, &[0xE000_0291, 0xE023_0291, 0xE085_4291, 0xE0C7_6291]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(1, 0xFFFF_FFFF); // -1
        cpu.regs.set(2, 3);
        for _ in 0..4 {
            cpu.step(&mut mem).unwrap();
        }
        assert_eq!(cpu.regs.get(0), 0xFFFF_FFFD);
        assert_eq!(cpu.regs.get(3), 0xFFFF_FFFA);
        assert_eq!((cpu.regs.get(5), cpu.regs.get(4)), (2, 0xFFFF_FFFD));
        assert_eq!(
            (cpu.regs.get(7), cpu.regs.get(6)),
            (0xFFFF_FFFF, 0xFFFF_FFFD)
        );
    }

    #[test]
    fn umlal_accumulates_and_sets_flags() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE0F1_0392]); // umlals r0, r1, r2, r3
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 1);
        cpu.regs.set(1, 0x8000_0000);
        cpu.regs.set(2, 2);
        cpu.regs.set(3, 3);
        cpu.step(&mut mem).unwrap();
        assert_eq!((cpu.regs.get(1), cpu.regs.get(0)), (0x8000_0000, 7));
        assert!(cpu.regs.cpsr.n() && !cpu.regs.cpsr.z());
    }
}
