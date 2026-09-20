//! THUMB load/store: formats 6–11, 14 and 15.

use crate::cpu::Cpu;
use crate::cpu::load;
use crate::cpu::registers::{LR, PC, SP};
use crate::memory::Memory;

#[inline]
const fn rd(op: u16) -> usize {
    (op & 0x7) as usize
}

#[inline]
const fn rb(op: u16) -> usize {
    ((op >> 3) & 0x7) as usize
}

#[inline]
const fn ro(op: u16) -> usize {
    ((op >> 6) & 0x7) as usize
}

impl Cpu {
    /// Format 6: `LDR Rd, [PC, #imm8*4]`. The PC used is word-aligned.
    pub(super) fn thumb_ldr_pc(&mut self, mem: &impl Memory, op: u16) -> u32 {
        let rd = usize::from((op >> 8) & 0x7);
        let address = (self.regs.get(PC) & !2).wrapping_add(u32::from(op & 0xFF) << 2);
        self.regs.set(rd, mem.read32(address));
        3
    }

    /// Format 7: `STR`/`STRB`/`LDR`/`LDRB Rd, [Rb, Ro]`.
    pub(super) fn thumb_ldr_str_reg(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let address = self.regs.get(rb(op)).wrapping_add(self.regs.get(ro(op)));
        let rd = rd(op);
        match (op >> 10) & 0b11 {
            0b00 => {
                mem.write32(address & !3, self.regs.get(rd));
                2
            }
            0b01 => {
                mem.write8(address, self.regs.get(rd) as u8);
                2
            }
            0b10 => {
                self.regs.set(rd, load::word(mem, address));
                3
            }
            _ => {
                self.regs.set(rd, u32::from(mem.read8(address)));
                3
            }
        }
    }

    /// Format 8: `STRH`/`LDRSB`/`LDRH`/`LDRSH Rd, [Rb, Ro]`.
    pub(super) fn thumb_ldr_str_sign_ext(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let address = self.regs.get(rb(op)).wrapping_add(self.regs.get(ro(op)));
        let rd = rd(op);
        match (op >> 10) & 0b11 {
            0b00 => {
                mem.write16(address & !1, self.regs.get(rd) as u16);
                2
            }
            0b01 => {
                self.regs.set(rd, load::signed_byte(mem, address));
                3
            }
            0b10 => {
                self.regs.set(rd, load::halfword(mem, address));
                3
            }
            _ => {
                self.regs.set(rd, load::signed_halfword(mem, address));
                3
            }
        }
    }

    /// Format 9: `STR`/`LDR`/`STRB`/`LDRB Rd, [Rb, #offset5]`.
    pub(super) fn thumb_ldr_str_imm(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let byte = op & (1 << 12) != 0;
        let load = op & (1 << 11) != 0;
        let offset = u32::from((op >> 6) & 0x1F);
        let offset = if byte { offset } else { offset << 2 };
        let address = self.regs.get(rb(op)).wrapping_add(offset);
        let rd = rd(op);
        match (load, byte) {
            (false, false) => {
                mem.write32(address & !3, self.regs.get(rd));
                2
            }
            (false, true) => {
                mem.write8(address, self.regs.get(rd) as u8);
                2
            }
            (true, false) => {
                self.regs.set(rd, load::word(mem, address));
                3
            }
            (true, true) => {
                self.regs.set(rd, u32::from(mem.read8(address)));
                3
            }
        }
    }

    /// Format 10: `STRH`/`LDRH Rd, [Rb, #offset5*2]`.
    pub(super) fn thumb_ldr_str_half(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let address = self
            .regs
            .get(rb(op))
            .wrapping_add(u32::from((op >> 6) & 0x1F) << 1);
        let rd = rd(op);
        if op & (1 << 11) != 0 {
            self.regs.set(rd, load::halfword(mem, address));
            3
        } else {
            mem.write16(address & !1, self.regs.get(rd) as u16);
            2
        }
    }

    /// Format 11: `STR`/`LDR Rd, [SP, #imm8*4]`.
    pub(super) fn thumb_ldr_str_sp(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let rd = usize::from((op >> 8) & 0x7);
        let address = self.regs.get(SP).wrapping_add(u32::from(op & 0xFF) << 2);
        if op & (1 << 11) != 0 {
            self.regs.set(rd, load::word(mem, address));
            3
        } else {
            mem.write32(address & !3, self.regs.get(rd));
            2
        }
    }

    /// Format 14: `PUSH {rlist, lr}` / `POP {rlist, pc}`.
    pub(super) fn thumb_push_pop(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let pop = op & (1 << 11) != 0;
        let extra = op & (1 << 8) != 0;
        let list = u32::from(op & 0xFF);
        let count = list.count_ones() + u32::from(extra);
        let mut cycles = 1;

        if pop {
            let mut address = self.regs.get(SP);
            for reg in 0..8 {
                if list & (1 << reg) != 0 {
                    self.regs.set(reg, mem.read32(address));
                    address = address.wrapping_add(4);
                    cycles += 1;
                }
            }
            if extra {
                // ARMv4T: POP pc stays in THUMB regardless of bit 0.
                self.set_pc(mem.read32(address));
                address = address.wrapping_add(4);
                cycles += 3;
            }
            self.regs.set(SP, address);
        } else {
            let mut address = self.regs.get(SP).wrapping_sub(count * 4);
            self.regs.set(SP, address);
            for reg in 0..8 {
                if list & (1 << reg) != 0 {
                    mem.write32(address, self.regs.get(reg));
                    address = address.wrapping_add(4);
                    cycles += 1;
                }
            }
            if extra {
                mem.write32(address, self.regs.get(LR));
                cycles += 1;
            }
        }
        cycles
    }

    /// Format 15: `STMIA`/`LDMIA Rb!, {rlist}`.
    pub(super) fn thumb_ldm_stm(&mut self, mem: &mut impl Memory, op: u16) -> u32 {
        let load = op & (1 << 11) != 0;
        let rb = usize::from((op >> 8) & 0x7);
        let list = u32::from(op & 0xFF);
        let base = self.regs.get(rb);
        let mut address = base;
        let mut cycles = 1;

        if list == 0 {
            // Empty-list quirk, as in ARM: r15 is transferred, base += 0x40.
            if load {
                self.set_pc(mem.read32(base));
            } else {
                mem.write32(base, self.regs.get(PC).wrapping_add(2));
            }
            self.regs.set(rb, base.wrapping_add(0x40));
            return cycles + 3;
        }

        let final_base = base.wrapping_add(list.count_ones() * 4);
        let mut first = true;
        for reg in 0..8 {
            if list & (1 << reg) == 0 {
                continue;
            }
            if load {
                self.regs.set(reg, mem.read32(address));
            } else {
                let value = if reg == rb && !first {
                    final_base
                } else {
                    self.regs.get(reg)
                };
                mem.write32(address, value);
            }
            address = address.wrapping_add(4);
            cycles += 1;
            first = false;
        }

        // LDMIA with the base in the list: the loaded value wins.
        if !(load && list & (1 << rb) != 0) {
            self.regs.set(rb, final_base);
        }
        cycles
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::registers::{LR, SP};
    use crate::cpu::test_util::{Ram, thumb_at};
    use crate::memory::Memory;

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
    fn ldr_pc_relative_is_word_aligned() {
        let mut mem = Ram::new();
        mem.write32(0x208, 0xCAFE_BABE);
        // at 0x202: ldr r0, [pc, #4] -> (0x206 & !2) + 4 = 0x208
        mem.load_thumb(0x202, &[0x4801]);
        let mut cpu = thumb_at(&mut mem, 0x202);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0xCAFE_BABE);
    }

    #[test]
    fn register_offset_transfers() {
        let mut mem = Ram::new();
        // str r0, [r1, r2] ; strb r0, [r1, r3] ; ldr r4, [r1, r2] ; ldrb r5, [r1, r3]
        let cpu = run(&mut mem, &[0x5088, 0x54C8, 0x588C, 0x5CCD], |c| {
            c.regs.set(0, 0x1122_3344);
            c.regs.set(1, 0x1000);
            c.regs.set(2, 4);
            c.regs.set(3, 9);
        });
        assert_eq!(mem.read32(0x1004), 0x1122_3344);
        assert_eq!(mem.read8(0x1009), 0x44);
        assert_eq!(cpu.regs.get(4), 0x1122_3344);
        assert_eq!(cpu.regs.get(5), 0x44);
    }

    #[test]
    fn sign_extending_transfers() {
        let mut mem = Ram::new();
        // strh r0, [r1, r2] ; ldrsb r3, [r1, r2] ; ldrh r4, [r1, r2] ; ldrsh r5, [r1, r2]
        let cpu = run(&mut mem, &[0x5288, 0x568B, 0x5A8C, 0x5E8D], |c| {
            c.regs.set(0, 0x1234_8180);
            c.regs.set(1, 0x1000);
            c.regs.set(2, 2);
        });
        assert_eq!(mem.read16(0x1002), 0x8180);
        assert_eq!(cpu.regs.get(3), 0xFFFF_FF80);
        assert_eq!(cpu.regs.get(4), 0x8180);
        assert_eq!(cpu.regs.get(5), 0xFFFF_8180);
    }

    #[test]
    fn immediate_offset_transfers() {
        let mut mem = Ram::new();
        // str r0, [r1, #8] ; ldr r2, [r1, #8] ; strb r0, [r1, #3] ; ldrb r3, [r1, #3]
        // strh r0, [r1, #6] ; ldrh r4, [r1, #6]
        let cpu = run(
            &mut mem,
            &[0x6088, 0x688A, 0x70C8, 0x78CB, 0x80C8, 0x88CC],
            |c| {
                c.regs.set(0, 0xAABB_CCDD);
                c.regs.set(1, 0x1000);
            },
        );
        assert_eq!(cpu.regs.get(2), 0xAABB_CCDD);
        assert_eq!(cpu.regs.get(3), 0xDD);
        assert_eq!(cpu.regs.get(4), 0xCCDD);
        assert_eq!(mem.read8(0x1003), 0xDD);
        assert_eq!(mem.read16(0x1006), 0xCCDD);
    }

    #[test]
    fn sp_relative_transfers() {
        let mut mem = Ram::new();
        // str r0, [sp, #4] ; ldr r1, [sp, #4]
        let cpu = run(&mut mem, &[0x9001, 0x9901], |c| {
            c.regs.set(0, 0x55);
            c.regs.set(SP, 0x1000);
        });
        assert_eq!(mem.read32(0x1004), 0x55);
        assert_eq!(cpu.regs.get(1), 0x55);
    }

    #[test]
    fn push_pop_round_trip_with_lr_pc() {
        let mut mem = Ram::new();
        // push {r0, r1, lr} ; pop {r2, r3, pc}
        let cpu = run(&mut mem, &[0xB503, 0xBD0C], |c| {
            c.regs.set(0, 10);
            c.regs.set(1, 11);
            c.regs.set(LR, 0x301);
            c.regs.set(SP, 0x1000);
        });
        assert_eq!(mem.read32(0xFF4), 10);
        assert_eq!(mem.read32(0xFF8), 11);
        assert_eq!(mem.read32(0xFFC), 0x301);
        assert_eq!(cpu.regs.get(SP), 0x1000);
        assert_eq!((cpu.regs.get(2), cpu.regs.get(3)), (10, 11));
        assert_eq!(cpu.next_pc(), 0x300);
        assert!(cpu.thumb(), "POP pc keeps THUMB on ARMv4T");
    }

    #[test]
    fn stmia_ldmia_with_writeback() {
        let mut mem = Ram::new();
        // stmia r0!, {r1, r2} ; ldmia r3!, {r4, r5}
        let cpu = run(&mut mem, &[0xC006, 0xCB30], |c| {
            c.regs.set(0, 0x1000);
            c.regs.set(1, 1);
            c.regs.set(2, 2);
            c.regs.set(3, 0x1000);
        });
        assert_eq!(cpu.regs.get(0), 0x1008);
        assert_eq!(cpu.regs.get(3), 0x1008);
        assert_eq!((cpu.regs.get(4), cpu.regs.get(5)), (1, 2));
    }

    #[test]
    fn ldmia_base_in_list_keeps_loaded_value() {
        let mut mem = Ram::new();
        mem.write32(0x1000, 0x77);
        let cpu = run(&mut mem, &[0xC801], |c| c.regs.set(0, 0x1000)); // ldmia r0!, {r0}
        assert_eq!(cpu.regs.get(0), 0x77);
    }

    #[test]
    fn stmia_empty_list_quirk() {
        let mut mem = Ram::new();
        let cpu = run(&mut mem, &[0xC000], |c| c.regs.set(0, 0x1000)); // stmia r0!, {}
        assert_eq!(mem.read32(0x1000), 0x206);
        assert_eq!(cpu.regs.get(0), 0x1040);
    }
}
