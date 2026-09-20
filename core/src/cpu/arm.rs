//! ARM (32-bit) instruction set: decoding and execution.

use crate::cpu::registers::{LR, PC};
use crate::cpu::{Cpu, Exception};
use crate::error::{GbaError, Result};

/// The instruction class an ARM opcode belongs to.
///
/// Decoding stops at the class; operand fields are extracted by the
/// executor, which already has the raw opcode in hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmKind {
    /// `BX Rn`
    BranchExchange,
    /// `B`/`BL label`
    Branch,
    /// `MRS`/`MSR`
    PsrTransfer,
    /// `AND`, `ADD`, `MOV`, `CMP`, … with register or immediate operand 2.
    DataProcessing,
    /// `MUL`/`MLA`
    Multiply,
    /// `UMULL`/`UMLAL`/`SMULL`/`SMLAL`
    MultiplyLong,
    /// `SWP`/`SWPB`
    SingleDataSwap,
    /// `LDRH`/`STRH`/`LDRSB`/`LDRSH`
    HalfwordTransfer,
    /// `LDR`/`STR` (word or byte)
    SingleDataTransfer,
    /// `LDM`/`STM`
    BlockDataTransfer,
    /// `SWI`
    SoftwareInterrupt,
    /// Coprocessor instructions: the GBA has no coprocessors, so these
    /// trap to the undefined-instruction vector.
    Coprocessor,
    /// Any encoding that is undefined on `ARMv4T`.
    Undefined,
}

/// Classifies an ARM opcode.
#[must_use]
pub const fn decode(op: u32) -> ArmKind {
    // Bits 27:25 select the coarse group; a few groups need bits 7:4 too.
    match (op >> 25) & 0b111 {
        0b000 => {
            if op & 0x0FFF_FFF0 == 0x012F_FF10 {
                return ArmKind::BranchExchange;
            }
            let bits7_4 = (op >> 4) & 0xF;
            if bits7_4 == 0b1001 {
                return match (op >> 23) & 0b11 {
                    0b00 => ArmKind::Multiply,
                    0b01 => ArmKind::MultiplyLong,
                    0b10 if op & 0x0030_0F00 == 0 => ArmKind::SingleDataSwap,
                    _ => ArmKind::Undefined,
                };
            }
            if bits7_4 & 0b1001 == 0b1001 {
                // bit 7 and bit 4 set with bits 6:5 != 00
                return ArmKind::HalfwordTransfer;
            }
            // Data processing with a TST/TEQ/CMP/CMN opcode and S=0 is
            // really MRS/MSR.
            if op & 0x0190_0000 == 0x0100_0000 {
                return ArmKind::PsrTransfer;
            }
            ArmKind::DataProcessing
        }
        0b001 => {
            if op & 0x0190_0000 == 0x0100_0000 {
                // Immediate MSR; the MRS slot here is undefined.
                if op & 0x0020_0000 != 0 {
                    ArmKind::PsrTransfer
                } else {
                    ArmKind::Undefined
                }
            } else {
                ArmKind::DataProcessing
            }
        }
        0b010 => ArmKind::SingleDataTransfer,
        // Register-offset LDR/STR with bit 4 set is the undefined space.
        0b011 => {
            if op & 0x10 != 0 {
                ArmKind::Undefined
            } else {
                ArmKind::SingleDataTransfer
            }
        }
        0b100 => ArmKind::BlockDataTransfer,
        0b101 => ArmKind::Branch,
        0b110 => ArmKind::Coprocessor,
        _ => {
            if op & 0x0100_0000 != 0 {
                ArmKind::SoftwareInterrupt
            } else {
                ArmKind::Coprocessor
            }
        }
    }
}

impl Cpu {
    /// Executes one ARM instruction. `address` is where it was fetched from.
    pub(super) fn execute_arm(&mut self, op: u32, address: u32) -> Result<u32> {
        if !self.regs.cpsr.condition(op >> 28) {
            return Ok(1);
        }

        match decode(op) {
            ArmKind::Branch => Ok(self.arm_branch(op)),
            ArmKind::BranchExchange => Ok(self.arm_branch_exchange(op)),
            ArmKind::SoftwareInterrupt => {
                self.enter_exception(Exception::SoftwareInterrupt);
                Ok(3)
            }
            ArmKind::Undefined | ArmKind::Coprocessor => {
                self.enter_exception(Exception::Undefined);
                Ok(3)
            }
            _ => Err(GbaError::UnimplementedInstruction {
                mode: "ARM",
                opcode: op,
                pc: address,
            }),
        }
    }

    /// `B`/`BL`: PC-relative branch with a 24-bit signed word offset.
    fn arm_branch(&mut self, op: u32) -> u32 {
        // Sign-extend bits 23:0 and scale to bytes.
        let offset = ((op << 8) as i32) >> 6;
        let pc = self.regs.get(PC);
        if op & (1 << 24) != 0 {
            self.regs.set(LR, pc.wrapping_sub(4));
        }
        self.set_pc(pc.wrapping_add(offset as u32));
        3
    }

    /// `BX Rn`: branch and optionally switch to THUMB (bit 0 of Rn).
    fn arm_branch_exchange(&mut self, op: u32) -> u32 {
        let target = self.regs.get((op & 0xF) as usize);
        self.regs.cpsr.set_thumb(target & 1 != 0);
        self.set_pc(target);
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_representative_encodings() {
        let cases = [
            (0xEA00_0000, ArmKind::Branch),             // b +0
            (0xEB00_0010, ArmKind::Branch),             // bl
            (0xE12F_FF10, ArmKind::BranchExchange),     // bx r0
            (0xE080_1002, ArmKind::DataProcessing),     // add r1, r0, r2
            (0xE3A0_0001, ArmKind::DataProcessing),     // mov r0, #1
            (0xE150_0001, ArmKind::DataProcessing),     // cmp r0, r1
            (0xE10F_0000, ArmKind::PsrTransfer),        // mrs r0, cpsr
            (0xE129_F000, ArmKind::PsrTransfer),        // msr cpsr_fc, r0
            (0xE328_F201, ArmKind::PsrTransfer),        // msr cpsr_f, #0x10000000
            (0xE000_0291, ArmKind::Multiply),           // mul r0, r1, r2
            (0xE081_0392, ArmKind::MultiplyLong),       // umull r0, r1, r2, r3
            (0xE100_0091, ArmKind::SingleDataSwap),     // swp r0, r1, [r0]
            (0xE1D0_00B0, ArmKind::HalfwordTransfer),   // ldrh r0, [r0]
            (0xE1D0_00D0, ArmKind::HalfwordTransfer),   // ldrsb r0, [r0]
            (0xE590_0000, ArmKind::SingleDataTransfer), // ldr r0, [r0]
            (0xE780_1002, ArmKind::SingleDataTransfer), // str r1, [r0, r2]
            (0xE780_1012, ArmKind::Undefined),          // reg offset with bit 4
            (0xE8BD_8000, ArmKind::BlockDataTransfer),  // ldmfd sp!, {pc}
            (0xEF00_0005, ArmKind::SoftwareInterrupt),  // swi 5
            (0xEE00_0000, ArmKind::Coprocessor),        // cdp
            (0xEC00_0000, ArmKind::Coprocessor),        // stc
        ];
        for (op, kind) in cases {
            assert_eq!(decode(op), kind, "opcode {op:#010x}");
        }
    }
}
