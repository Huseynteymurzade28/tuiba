//! THUMB register/immediate arithmetic: formats 1–5, 12 and 13.

use crate::cpu::Cpu;
use crate::cpu::alu::{ShiftType, add_with_carry, multiply_cycles, shift_imm, shift_reg};
use crate::cpu::registers::{PC, SP};

#[inline]
const fn rd(op: u16) -> usize {
    (op & 0x7) as usize
}

#[inline]
const fn rs(op: u16) -> usize {
    ((op >> 3) & 0x7) as usize
}

impl Cpu {
    /// Stores an arithmetic result and sets all four flags.
    #[inline]
    fn set_arith(&mut self, rd: usize, result: (u32, bool, bool)) {
        self.regs.set(rd, result.0);
        self.set_arith_flags(result);
    }

    #[inline]
    fn set_arith_flags(&mut self, (value, c, v): (u32, bool, bool)) {
        self.regs.cpsr.set_nz(value);
        self.regs.cpsr.set_c(c);
        self.regs.cpsr.set_v(v);
    }

    /// Stores a logical/shift result and sets N, Z and C.
    #[inline]
    fn set_logical(&mut self, rd: usize, value: u32, carry: bool) {
        self.regs.set(rd, value);
        self.regs.cpsr.set_nz(value);
        self.regs.cpsr.set_c(carry);
    }

    /// Format 1: `LSL`/`LSR`/`ASR Rd, Rs, #offset5`.
    pub(super) fn thumb_move_shifted(&mut self, op: u16) -> u32 {
        let kind = ShiftType::from_bits(u32::from(op >> 11));
        let amount = u32::from((op >> 6) & 0x1F);
        let (value, carry) = shift_imm(kind, self.regs.get(rs(op)), amount, self.regs.cpsr.c());
        self.set_logical(rd(op), value, carry);
        0
    }

    /// Format 2: `ADD`/`SUB Rd, Rs, Rn|#imm3`.
    pub(super) fn thumb_add_sub(&mut self, op: u16) -> u32 {
        let operand = u32::from((op >> 6) & 0x7);
        let operand = if op & (1 << 10) != 0 {
            operand
        } else {
            self.regs.get(operand as usize)
        };
        let a = self.regs.get(rs(op));
        let result = if op & (1 << 9) != 0 {
            add_with_carry(a, !operand, true)
        } else {
            add_with_carry(a, operand, false)
        };
        self.set_arith(rd(op), result);
        0
    }

    /// Format 3: `MOV`/`CMP`/`ADD`/`SUB Rd, #imm8`.
    pub(super) fn thumb_immediate(&mut self, op: u16) -> u32 {
        let rd = usize::from((op >> 8) & 0x7);
        let imm = u32::from(op & 0xFF);
        let a = self.regs.get(rd);
        match (op >> 11) & 0b11 {
            0b00 => {
                self.regs.set(rd, imm);
                self.regs.cpsr.set_nz(imm);
            }
            0b01 => self.set_arith_flags(add_with_carry(a, !imm, true)),
            0b10 => self.set_arith(rd, add_with_carry(a, imm, false)),
            _ => self.set_arith(rd, add_with_carry(a, !imm, true)),
        }
        0
    }

    /// Format 4: register-register ALU operations.
    pub(super) fn thumb_alu(&mut self, op: u16) -> u32 {
        let rd = rd(op);
        let a = self.regs.get(rd);
        let b = self.regs.get(rs(op));
        let carry = self.regs.cpsr.c();
        let mut cycles = 0;

        let shift = |kind: ShiftType| shift_reg(kind, a, b, carry);
        match (op >> 6) & 0xF {
            0x0 => self.set_logical(rd, a & b, carry),
            0x1 => self.set_logical(rd, a ^ b, carry),
            0x2 => {
                let (v, c) = shift(ShiftType::Lsl);
                self.set_logical(rd, v, c);
                cycles = 1;
            }
            0x3 => {
                let (v, c) = shift(ShiftType::Lsr);
                self.set_logical(rd, v, c);
                cycles = 1;
            }
            0x4 => {
                let (v, c) = shift(ShiftType::Asr);
                self.set_logical(rd, v, c);
                cycles = 1;
            }
            0x5 => self.set_arith(rd, add_with_carry(a, b, carry)),
            0x6 => self.set_arith(rd, add_with_carry(a, !b, carry)),
            0x7 => {
                let (v, c) = shift(ShiftType::Ror);
                self.set_logical(rd, v, c);
                cycles = 1;
            }
            0x8 => {
                self.regs.cpsr.set_nz(a & b);
                self.regs.cpsr.set_c(carry);
            }
            0x9 => self.set_arith(rd, add_with_carry(0, !b, true)),
            0xA => self.set_arith_flags(add_with_carry(a, !b, true)),
            0xB => self.set_arith_flags(add_with_carry(a, b, false)),
            0xC => self.set_logical(rd, a | b, carry),
            0xD => {
                let v = a.wrapping_mul(b);
                self.regs.set(rd, v);
                self.regs.cpsr.set_nz(v);
                cycles = multiply_cycles(b, true);
            }
            0xE => self.set_logical(rd, a & !b, carry),
            _ => self.set_logical(rd, !b, carry),
        }
        cycles
    }

    /// Format 5: `ADD`/`CMP`/`MOV` on high registers and `BX`.
    pub(super) fn thumb_hi_reg(&mut self, op: u16) -> u32 {
        let rd = usize::from(((op >> 4) & 0x8) | (op & 0x7));
        let rs = usize::from((op >> 3) & 0xF);
        let a = self.regs.get(rd);
        let b = self.regs.get(rs);
        match (op >> 8) & 0b11 {
            0b00 => self.thumb_write_maybe_pc(rd, a.wrapping_add(b)),
            0b01 => {
                self.set_arith_flags(add_with_carry(a, !b, true));
                0
            }
            0b10 => self.thumb_write_maybe_pc(rd, b),
            _ => {
                self.regs.cpsr.set_thumb(b & 1 != 0);
                self.set_pc(b);
                0
            }
        }
    }

    /// Writes `value` to `rd`, flushing when `rd` is r15 (staying in THUMB).
    #[inline]
    fn thumb_write_maybe_pc(&mut self, rd: usize, value: u32) -> u32 {
        if rd == PC {
            self.set_pc(value);
            0
        } else {
            self.regs.set(rd, value);
            0
        }
    }

    /// Format 12: `ADD Rd, PC|SP, #imm8*4`.
    pub(super) fn thumb_load_address(&mut self, op: u16) -> u32 {
        let rd = usize::from((op >> 8) & 0x7);
        let imm = u32::from(op & 0xFF) << 2;
        let base = if op & (1 << 11) != 0 {
            self.regs.get(SP)
        } else {
            self.regs.get(PC) & !2
        };
        self.regs.set(rd, base.wrapping_add(imm));
        0
    }

    /// Format 13: `ADD SP, #±imm7*4`.
    pub(super) fn thumb_add_sp(&mut self, op: u16) -> u32 {
        let imm = u32::from(op & 0x7F) << 2;
        let sp = self.regs.get(SP);
        let sp = if op & (1 << 7) != 0 {
            sp.wrapping_sub(imm)
        } else {
            sp.wrapping_add(imm)
        };
        self.regs.set(SP, sp);
        0
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::registers::{LR, SP};
    use crate::cpu::test_util::{Ram, thumb_at};

    fn run(
        mem: &mut Ram,
        ops: &[u16],
        setup: impl FnOnce(&mut crate::cpu::Cpu),
    ) -> crate::cpu::Cpu {
        mem.load_thumb(0x200, ops);
        let mut cpu = thumb_at(mem, 0x200);
        setup(&mut cpu);
        for _ in 0..ops.len() {
            cpu.step(mem);
        }
        cpu
    }

    #[test]
    fn move_shifted() {
        let mut mem = Ram::new();
        // lsl r0, r1, #4 ; lsr r2, r1, #0 (=32) ; asr r3, r1, #1
        let cpu = run(&mut mem, &[0x0108, 0x080A, 0x104B], |c| {
            c.regs.set(1, 0x8000_0001);
        });
        assert_eq!(cpu.regs.get(0), 0x10);
        assert_eq!(cpu.regs.get(2), 0);
        assert_eq!(cpu.regs.get(3), 0xC000_0000);
        assert!(cpu.regs.cpsr.c() && cpu.regs.cpsr.n());
    }

    #[test]
    fn add_sub_register_and_imm3() {
        let mut mem = Ram::new();
        // add r0, r1, r2 ; sub r3, r1, #1 ; sub r4, r1, r1
        let cpu = run(&mut mem, &[0x1888, 0x1E4B, 0x1A4C], |c| {
            c.regs.set(1, 5);
            c.regs.set(2, 7);
        });
        assert_eq!(cpu.regs.get(0), 12);
        assert_eq!(cpu.regs.get(3), 4);
        assert_eq!(cpu.regs.get(4), 0);
        assert!(cpu.regs.cpsr.z() && cpu.regs.cpsr.c());
    }

    #[test]
    fn immediate_ops() {
        let mut mem = Ram::new();
        // mov r0, #200 ; add r0, #100 ; sub r0, #44 ; cmp r0, #0
        let cpu = run(&mut mem, &[0x20C8, 0x3064, 0x382C, 0x2800], |_| {});
        assert_eq!(cpu.regs.get(0), 256);
        assert!(!cpu.regs.cpsr.z() && cpu.regs.cpsr.c());
    }

    #[test]
    fn alu_register_ops() {
        let mut mem = Ram::new();
        // and r0, r1 ; lsl r2, r3 ; neg r4, r1 ; mul r5, r1 ; tst r1, r1 ; mvn r6, r1
        let cpu = run(
            &mut mem,
            &[0x4008, 0x409A, 0x424C, 0x434D, 0x4209, 0x43CE],
            |c| {
                c.regs.set(0, 0xFF);
                c.regs.set(1, 0x0F);
                c.regs.set(2, 1);
                c.regs.set(3, 33);
                c.regs.set(5, 3);
            },
        );
        assert_eq!(cpu.regs.get(0), 0x0F);
        assert_eq!(cpu.regs.get(2), 0, "lsl by 33 clears");
        assert_eq!(cpu.regs.get(4), 0xFFFF_FFF1);
        assert_eq!(cpu.regs.get(5), 45);
        assert_eq!(cpu.regs.get(6), 0xFFFF_FFF0);
        assert!(cpu.regs.cpsr.n());
    }

    #[test]
    fn adc_sbc_use_carry() {
        let mut mem = Ram::new();
        // adc r0, r1 ; sbc r2, r1
        let cpu = run(&mut mem, &[0x4148, 0x418A], |c| {
            c.regs.set(0, 1);
            c.regs.set(1, 1);
            c.regs.set(2, 5);
            c.regs.cpsr.set_c(true);
        });
        assert_eq!(cpu.regs.get(0), 3);
        assert_eq!(cpu.regs.get(2), 3, "sbc subtracts the borrow left by adc");
    }

    #[test]
    fn hi_register_ops() {
        let mut mem = Ram::new();
        // add r0, r8 ; mov r9, r0 ; cmp r9, r0 ; mov r1, pc
        let cpu = run(&mut mem, &[0x4440, 0x4681, 0x4581, 0x4679], |c| {
            c.regs.set(8, 0x100);
        });
        assert_eq!(cpu.regs.get(0), 0x100);
        assert_eq!(cpu.regs.get(9), 0x100);
        assert!(cpu.regs.cpsr.z());
        assert_eq!(cpu.regs.get(1), 0x20A, "PC+4 of the instruction at 0x206");
    }

    #[test]
    fn hi_register_add_to_pc_branches() {
        let mut mem = Ram::new();
        mem.load_thumb(0x200, &[0x4487]); // add pc, r0
        let mut cpu = thumb_at(&mut mem, 0x200);
        cpu.regs.set(0, 0x101);
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x304, "0x204 + 0x101, bit 0 dropped");
        assert!(cpu.thumb());
    }

    #[test]
    fn bx_lr_returns_to_arm() {
        let mut mem = Ram::new();
        mem.load_thumb(0x200, &[0x4770]); // bx lr
        let mut cpu = thumb_at(&mut mem, 0x200);
        cpu.regs.set(LR, 0x400);
        cpu.step(&mut mem);
        assert!(!cpu.thumb());
        assert_eq!(cpu.next_pc(), 0x400);
    }

    #[test]
    fn load_address_and_add_sp() {
        let mut mem = Ram::new();
        // add r0, pc, #8 ; add r1, sp, #16 ; add sp, #-8 ; add sp, #4
        let cpu = run(&mut mem, &[0xA002, 0xA904, 0xB082, 0xB001], |c| {
            c.regs.set(SP, 0x1000);
        });
        assert_eq!(cpu.regs.get(0), 0x20C);
        assert_eq!(cpu.regs.get(1), 0x1010);
        assert_eq!(cpu.regs.get(SP), 0xFFC);
    }

    #[test]
    fn load_address_from_pc_clears_bit1() {
        let mut mem = Ram::new();
        mem.load_thumb(0x202, &[0xA000]); // add r0, pc, #0 at 0x202 -> (0x206 & !2)
        let mut cpu = thumb_at(&mut mem, 0x202);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0x204);
    }
}
