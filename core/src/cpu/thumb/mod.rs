//! THUMB (16-bit) instruction set: decoding and execution.

mod alu;
mod mem;

use crate::cpu::registers::{LR, PC};
use crate::cpu::{Cpu, Exception};
use crate::memory::Memory;

/// The THUMB instruction format (numbered as in the ARM7TDMI manual).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbKind {
    /// 1: `LSL`/`LSR`/`ASR Rd, Rs, #imm`
    MoveShifted,
    /// 2: `ADD`/`SUB Rd, Rs, Rn|#imm3`
    AddSub,
    /// 3: `MOV`/`CMP`/`ADD`/`SUB Rd, #imm8`
    MovCmpAddSubImm,
    /// 4: register-register ALU operations
    AluOps,
    /// 5: `ADD`/`CMP`/`MOV` with high registers, and `BX`
    HiRegBx,
    /// 6: `LDR Rd, [PC, #imm]`
    LdrPcRel,
    /// 7: `LDR`/`STR`(`B`) `Rd, [Rb, Ro]`
    LdrStrReg,
    /// 8: `LDRH`/`STRH`/`LDRSB`/`LDRSH Rd, [Rb, Ro]`
    LdrStrSignExt,
    /// 9: `LDR`/`STR`(`B`) `Rd, [Rb, #imm]`
    LdrStrImm,
    /// 10: `LDRH`/`STRH Rd, [Rb, #imm]`
    LdrStrHalf,
    /// 11: `LDR`/`STR Rd, [SP, #imm]`
    LdrStrSpRel,
    /// 12: `ADD Rd, PC|SP, #imm`
    LoadAddress,
    /// 13: `ADD SP, #±imm`
    AddSp,
    /// 14: `PUSH`/`POP`
    PushPop,
    /// 15: `LDMIA`/`STMIA`
    LdmStm,
    /// 16: `B<cond> label`
    CondBranch,
    /// 17: `SWI`
    Swi,
    /// 18: `B label`
    Branch,
    /// 19: `BL label` (two halves)
    LongBranchLink,
    /// Unallocated encoding.
    Undefined,
}

/// Classifies a THUMB opcode.
#[must_use]
pub const fn decode(op: u16) -> ThumbKind {
    match op >> 13 {
        0b000 => {
            if (op >> 11) & 0b11 == 0b11 {
                ThumbKind::AddSub
            } else {
                ThumbKind::MoveShifted
            }
        }
        0b001 => ThumbKind::MovCmpAddSubImm,
        0b010 => match (op >> 10) & 0b111 {
            0b000 => ThumbKind::AluOps,
            0b001 => ThumbKind::HiRegBx,
            0b010 | 0b011 => ThumbKind::LdrPcRel,
            _ => {
                if op & (1 << 9) != 0 {
                    ThumbKind::LdrStrSignExt
                } else {
                    ThumbKind::LdrStrReg
                }
            }
        },
        0b011 => ThumbKind::LdrStrImm,
        0b100 => {
            if op & (1 << 12) != 0 {
                ThumbKind::LdrStrSpRel
            } else {
                ThumbKind::LdrStrHalf
            }
        }
        0b101 => {
            if op & (1 << 12) == 0 {
                ThumbKind::LoadAddress
            } else if op & 0x0F00 == 0 {
                ThumbKind::AddSp
            } else if op & 0x0600 == 0x0400 {
                ThumbKind::PushPop
            } else {
                ThumbKind::Undefined
            }
        }
        0b110 => {
            if op & (1 << 12) == 0 {
                ThumbKind::LdmStm
            } else if op & 0x0F00 == 0x0F00 {
                ThumbKind::Swi
            } else if op & 0x0F00 == 0x0E00 {
                ThumbKind::Undefined
            } else {
                ThumbKind::CondBranch
            }
        }
        _ => match (op >> 11) & 0b11 {
            0b00 => ThumbKind::Branch,
            0b01 => ThumbKind::Undefined,
            _ => ThumbKind::LongBranchLink,
        },
    }
}

impl Cpu {
    /// Executes one THUMB instruction.
    pub(super) fn execute_thumb(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        match decode(op) {
            ThumbKind::MoveShifted => self.thumb_move_shifted(op),
            ThumbKind::AddSub => self.thumb_add_sub(op),
            ThumbKind::MovCmpAddSubImm => self.thumb_immediate(op),
            ThumbKind::AluOps => self.thumb_alu(op),
            ThumbKind::HiRegBx => self.thumb_hi_reg(op),
            ThumbKind::LdrPcRel => self.thumb_ldr_pc(mem, op),
            ThumbKind::LdrStrReg => self.thumb_ldr_str_reg(mem, op),
            ThumbKind::LdrStrSignExt => self.thumb_ldr_str_sign_ext(mem, op),
            ThumbKind::LdrStrImm => self.thumb_ldr_str_imm(mem, op),
            ThumbKind::LdrStrHalf => self.thumb_ldr_str_half(mem, op),
            ThumbKind::LdrStrSpRel => self.thumb_ldr_str_sp(mem, op),
            ThumbKind::LoadAddress => self.thumb_load_address(op),
            ThumbKind::AddSp => self.thumb_add_sp(op),
            ThumbKind::PushPop => self.thumb_push_pop(mem, op),
            ThumbKind::LdmStm => self.thumb_ldm_stm(mem, op),
            ThumbKind::CondBranch => self.thumb_cond_branch(op),
            ThumbKind::Branch => self.thumb_branch(op),
            ThumbKind::LongBranchLink => self.thumb_long_branch_link(op),
            ThumbKind::Swi => {
                self.enter_exception(Exception::SoftwareInterrupt);
                3
            }
            ThumbKind::Undefined => {
                self.enter_exception(Exception::Undefined);
                3
            }
        }
    }

    /// Format 18: unconditional branch, 11-bit signed halfword offset.
    fn thumb_branch(&mut self, op: u16) -> u32 {
        let offset = ((i32::from(op) << 21) >> 20) as u32;
        self.set_pc(self.regs.get(PC).wrapping_add(offset));
        3
    }

    /// Format 16: conditional branch, 8-bit signed halfword offset.
    fn thumb_cond_branch(&mut self, op: u16) -> u32 {
        if !self.regs.cpsr.condition(u32::from(op >> 8)) {
            return 1;
        }
        let offset = (i32::from(op as i8) << 1) as u32;
        self.set_pc(self.regs.get(PC).wrapping_add(offset));
        3
    }

    /// Format 19: `BL` split into two halfwords. The first stashes the
    /// upper offset in LR; the second completes the branch and sets LR to
    /// the return address with bit 0 set (THUMB).
    fn thumb_long_branch_link(&mut self, op: u16) -> u32 {
        let offset = u32::from(op & 0x7FF);
        if op & (1 << 11) == 0 {
            let upper = ((offset << 21) as i32 >> 9) as u32;
            self.regs.set(LR, self.regs.get(PC).wrapping_add(upper));
            1
        } else {
            let return_addr = self.regs.get(PC).wrapping_sub(2);
            let target = self.regs.get(LR).wrapping_add(offset << 1);
            self.regs.set(LR, return_addr | 1);
            self.set_pc(target);
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_representative_encodings() {
        let cases = [
            (0x0000, ThumbKind::MoveShifted),     // lsl r0, r0, #0
            (0x1840, ThumbKind::AddSub),          // add r0, r0, r1
            (0x2001, ThumbKind::MovCmpAddSubImm), // mov r0, #1
            (0x4008, ThumbKind::AluOps),          // and r0, r1
            (0x4700, ThumbKind::HiRegBx),         // bx r0
            (0x4478, ThumbKind::HiRegBx),         // add r0, pc
            (0x4800, ThumbKind::LdrPcRel),        // ldr r0, [pc, #0]
            (0x5040, ThumbKind::LdrStrReg),       // str r0, [r0, r1]
            (0x5240, ThumbKind::LdrStrSignExt),   // strh r0, [r0, r1]
            (0x6800, ThumbKind::LdrStrImm),       // ldr r0, [r0]
            (0x8800, ThumbKind::LdrStrHalf),      // ldrh r0, [r0]
            (0x9000, ThumbKind::LdrStrSpRel),     // str r0, [sp]
            (0xA000, ThumbKind::LoadAddress),     // add r0, pc, #0
            (0xB080, ThumbKind::AddSp),           // sub sp, #0
            (0xB500, ThumbKind::PushPop),         // push {lr}
            (0xBD00, ThumbKind::PushPop),         // pop {pc}
            (0xB100, ThumbKind::Undefined),
            (0xC001, ThumbKind::LdmStm),     // stmia r0!, {r0}
            (0xD0FE, ThumbKind::CondBranch), // beq -2
            (0xDE00, ThumbKind::Undefined),
            (0xDF05, ThumbKind::Swi),    // swi 5
            (0xE7FE, ThumbKind::Branch), // b .
            (0xE800, ThumbKind::Undefined),
            (0xF000, ThumbKind::LongBranchLink),
            (0xF800, ThumbKind::LongBranchLink),
        ];
        for (op, kind) in cases {
            assert_eq!(decode(op), kind, "opcode {op:#06x}");
        }
    }
}
