//! ARM load/store instructions: single, halfword/signed, block and swap.

use crate::cpu::alu::{ShiftType, shift_imm};
use crate::cpu::load;
use crate::cpu::registers::PC;
use crate::cpu::{Cpu, Exception};
use crate::memory::Memory;

impl Cpu {
    /// Reads `Rn` as a base register; r15 reads as PC+8 like everywhere.
    #[inline]
    fn base(&self, rn: usize) -> u32 {
        self.regs.get(rn)
    }

    /// Value of `Rd` for a store; r15 stores as PC+12.
    #[inline]
    fn store_value(&self, rd: usize) -> u32 {
        let value = self.regs.get(rd);
        if rd == PC {
            value.wrapping_add(4)
        } else {
            value
        }
    }

    /// Applies pre/post indexing and writeback shared by the single and
    /// halfword transfers. Returns the effective address to access.
    ///
    /// `pre`: bit 24, `up`: bit 23, `writeback`: bit 21 (or post-indexed).
    #[inline]
    fn index(&mut self, op: u32, rn: usize, offset: u32) -> u32 {
        let base = self.base(rn);
        let up = op & (1 << 23) != 0;
        let pre = op & (1 << 24) != 0;
        let offset_base = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if pre { offset_base } else { base };
        if !pre || op & (1 << 21) != 0 {
            self.regs.set(rn, offset_base);
        }
        address
    }

    /// `LDR`/`STR`, word or byte.
    pub(super) fn arm_single_transfer(&mut self, mem: &mut impl Memory, op: u32) -> u32 {
        let load = op & (1 << 20) != 0;
        let byte = op & (1 << 22) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;

        let offset = if op & (1 << 25) == 0 {
            op & 0xFFF
        } else {
            let rm = self.regs.get((op & 0xF) as usize);
            let kind = ShiftType::from_bits(op >> 5);
            shift_imm(kind, rm, (op >> 7) & 0x1F, self.regs.cpsr.c()).0
        };

        // For stores the value must be read before any writeback to Rn.
        let value = self.store_value(rd);
        let address = self.index(op, rn, offset);

        if load {
            let value = if byte {
                u32::from(mem.read8(address))
            } else {
                load::word(mem, address)
            };
            if rd == PC {
                self.set_pc(value);
            } else {
                self.regs.set(rd, value);
            }
            1
        } else {
            if byte {
                mem.write8(address, value as u8);
            } else {
                mem.write32(address & !3, value);
            }
            0
        }
    }

    /// `LDRH`/`STRH`/`LDRSB`/`LDRSH`.
    pub(super) fn arm_halfword_transfer(&mut self, mem: &mut impl Memory, op: u32) -> u32 {
        let load = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;
        let offset = if op & (1 << 22) != 0 {
            ((op >> 4) & 0xF0) | (op & 0xF)
        } else {
            self.regs.get((op & 0xF) as usize)
        };

        let value = self.store_value(rd);
        let address = self.index(op, rn, offset);

        match ((op >> 5) & 0b11, load) {
            (0b01, false) => {
                mem.write16(address & !1, value as u16);
                0
            }
            (0b01, true) => self.finish_load(rd, load::halfword(mem, address)),
            (0b10, true) => self.finish_load(rd, load::signed_byte(mem, address)),
            (0b11, true) => self.finish_load(rd, load::signed_halfword(mem, address)),
            // LDRD/STRD slots: undefined on ARMv4T.
            _ => {
                self.enter_exception(Exception::Undefined);
                0
            }
        }
    }

    #[inline]
    fn finish_load(&mut self, rd: usize, value: u32) -> u32 {
        if rd == PC {
            self.set_pc(value);
        } else {
            self.regs.set(rd, value);
        }
        1
    }

    /// `LDM`/`STM` in all four addressing modes, with the S-bit user-bank
    /// and SPSR-restore forms.
    pub(super) fn arm_block_transfer(&mut self, mem: &mut impl Memory, op: u32) -> u32 {
        let load = op & (1 << 20) != 0;
        let writeback = op & (1 << 21) != 0;
        let s_bit = op & (1 << 22) != 0;
        let up = op & (1 << 23) != 0;
        let pre = op & (1 << 24) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let mut list = op & 0xFFFF;

        // Empty list quirk: r15 is transferred and the base moves by 0x40.
        let empty = list == 0;
        if empty {
            list = 1 << PC;
        }
        let count = if empty { 16 } else { list.count_ones() };
        let span = count * 4;

        let base = self.base(rn) & !3;
        let final_base = if up {
            base.wrapping_add(span)
        } else {
            base.wrapping_sub(span)
        };
        // Registers always occupy ascending addresses from the lowest one.
        let mut address = match (up, pre) {
            (true, false) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => final_base.wrapping_add(4),
            (false, true) => final_base,
        };

        let pc_in_list = list & (1 << PC) != 0;
        // S without r15 (or any STM with S) transfers the User bank.
        let user_bank = s_bit && !(load && pc_in_list);

        let cycles = u32::from(load);
        let mut first = true;
        for reg in 0..16 {
            if list & (1 << reg) == 0 {
                continue;
            }
            if load {
                let value = mem.read32(address);
                if reg == PC {
                    self.set_pc(value);
                } else if user_bank {
                    self.regs.set_user(reg, value);
                } else {
                    self.regs.set(reg, value);
                }
            } else {
                let value = if user_bank {
                    self.regs.get_user(reg)
                } else if reg == PC {
                    self.store_value(PC)
                } else if reg == rn && !first && writeback {
                    // Writeback lands after the first store, so a later
                    // occurrence of the base sees the updated value.
                    final_base
                } else {
                    self.regs.get(reg)
                };
                mem.write32(address, value);
            }
            address = address.wrapping_add(4);
            first = false;
        }

        if load
            && s_bit
            && pc_in_list
            && let Some(spsr) = self.regs.spsr()
        {
            self.regs.set_cpsr(spsr);
        }

        // A loaded base wins over writeback; an empty list always writes back.
        let base_loaded = load && list & (1 << rn) != 0 && !empty;
        if (writeback || empty) && !base_loaded {
            let final_base = if empty {
                if up {
                    base.wrapping_add(0x40)
                } else {
                    base.wrapping_sub(0x40)
                }
            } else {
                final_base
            };
            self.regs.set(rn, final_base);
        }
        cycles
    }

    /// `SWP`/`SWPB`: atomic exchange of `Rm` with memory at `[Rn]` into `Rd`.
    pub(super) fn arm_swap(&mut self, mem: &mut impl Memory, op: u32) -> u32 {
        let byte = op & (1 << 22) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;
        let rm = (op & 0xF) as usize;
        let address = self.regs.get(rn);
        let source = self.regs.get(rm);
        let old = if byte {
            let old = u32::from(mem.read8(address));
            mem.write8(address, source as u8);
            old
        } else {
            let old = load::word(mem, address);
            mem.write32(address & !3, source);
            old
        };
        self.regs.set(rd, old);
        1
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::Mode;
    use crate::cpu::registers::{LR, PC, SP};
    use crate::cpu::test_util::{Ram, arm_at};
    use crate::memory::Memory;

    #[test]
    fn ldr_str_immediate_offsets() {
        let mut mem = Ram::new();
        // str r1, [r0, #4] ; ldr r2, [r0, #4] ; ldrb r3, [r0, #5] ; strb r1, [r0, #8]
        mem.load_arm(0x100, &[0xE580_1004, 0xE590_2004, 0xE5D0_3005, 0xE5C0_1008]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.regs.set(1, 0xAABB_CCDD);
        for _ in 0..4 {
            cpu.step(&mut mem);
        }
        assert_eq!(mem.read32(0x1004), 0xAABB_CCDD);
        assert_eq!(cpu.regs.get(2), 0xAABB_CCDD);
        assert_eq!(cpu.regs.get(3), 0xCC);
        assert_eq!(mem.read32(0x1008), 0xDD);
        assert_eq!(cpu.regs.get(0), 0x1000, "no writeback without W");
    }

    #[test]
    fn pre_and_post_indexing_with_writeback() {
        let mut mem = Ram::new();
        // str r1, [r0, #4]! ; str r1, [r0], #-4 ; ldr r2, [r0, -r3, lsl #2]!
        mem.load_arm(0x100, &[0xE5A0_1004, 0xE400_1004, 0xE730_2103]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.regs.set(1, 1);
        cpu.regs.set(3, 1);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0x1004);
        assert_eq!(mem.read32(0x1004), 1);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0x1000);
        assert_eq!(mem.read32(0x1004), 1);
        mem.write32(0xFFC, 0x77);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0xFFC);
        assert_eq!(cpu.regs.get(2), 0x77);
    }

    #[test]
    fn ldr_misaligned_rotates_and_pc_relative() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE59F_0000, 0xE1A0_0000, 0x1234_5678, 0xE590_1001]);
        // ldr r0, [pc] -> loads word at 0x108 ; nop ; literal ; ldr r1, [r0, #1]
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0x1234_5678);
        cpu.regs.set(0, 0x108);
        cpu.flush_pipeline(&mem, 0x10C);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(1), 0x7812_3456);
    }

    #[test]
    fn str_pc_stores_plus_12_and_ldr_pc_branches() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE580_F000, 0xE590_F000]); // str pc, [r0] ; ldr pc, [r0]
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.step(&mut mem);
        assert_eq!(mem.read32(0x1000), 0x10C);
        mem.write32(0x1000, 0x203);
        cpu.step(&mut mem);
        assert_eq!(cpu.next_pc(), 0x200);
        assert!(!cpu.thumb(), "ARMv4 LDR pc does not switch state");
    }

    #[test]
    fn halfword_and_signed_transfers() {
        let mut mem = Ram::new();
        // strh r1, [r0] ; ldrh r2, [r0] ; ldrsh r3, [r0] ; ldrsb r4, [r0, #1] ; ldrh r5, [r0, r6]
        mem.load_arm(
            0x100,
            &[
                0xE1C0_10B0,
                0xE1D0_20B0,
                0xE1D0_30F0,
                0xE1D0_40D1,
                0xE190_50B6,
            ],
        );
        let mut mem_cpu = arm_at(&mut mem, 0x100);
        let cpu = &mut mem_cpu;
        cpu.regs.set(0, 0x1000);
        cpu.regs.set(1, 0xFFFF_8001);
        cpu.regs.set(6, 1);
        for _ in 0..5 {
            cpu.step(&mut mem);
        }
        assert_eq!(mem.read16(0x1000), 0x8001);
        assert_eq!(cpu.regs.get(2), 0x8001);
        assert_eq!(cpu.regs.get(3), 0xFFFF_8001);
        assert_eq!(cpu.regs.get(4), 0xFFFF_FF80);
        assert_eq!(cpu.regs.get(5), 0x0100_0080, "misaligned LDRH rotates");
    }

    #[test]
    fn push_pop_via_stmfd_ldmfd() {
        let mut mem = Ram::new();
        // stmfd sp!, {r0-r2, lr} ; ldmfd sp!, {r4-r6, pc}
        mem.load_arm(0x100, &[0xE92D_4007, 0xE8BD_8070]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(SP, 0x2000);
        cpu.regs.set(0, 10);
        cpu.regs.set(1, 11);
        cpu.regs.set(2, 12);
        cpu.regs.set(LR, 0x300);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(SP), 0x1FF0);
        assert_eq!(mem.read32(0x1FF0), 10);
        assert_eq!(mem.read32(0x1FF4), 11);
        assert_eq!(mem.read32(0x1FF8), 12);
        assert_eq!(mem.read32(0x1FFC), 0x300);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(SP), 0x2000);
        assert_eq!(
            (cpu.regs.get(4), cpu.regs.get(5), cpu.regs.get(6)),
            (10, 11, 12)
        );
        assert_eq!(cpu.next_pc(), 0x300);
    }

    #[test]
    fn all_four_addressing_modes_store_ascending() {
        let mut mem = Ram::new();
        // stmia r0, {r1,r2} ; stmib r0, {r1,r2} ; stmda r0, {r1,r2} ; stmdb r0, {r1,r2}
        mem.load_arm(0x100, &[0xE880_0006, 0xE980_0006, 0xE800_0006, 0xE900_0006]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(1, 1);
        cpu.regs.set(2, 2);
        let expected = [
            (0x1000, 0x1004),
            (0x1004, 0x1008),
            (0xFFC, 0x1000),
            (0xFF8, 0xFFC),
        ];
        for (lo, hi) in expected {
            cpu.regs.set(0, 0x1000);
            mem.bytes[0xFF0..0x1010].fill(0);
            cpu.step(&mut mem);
            assert_eq!(mem.read32(lo), 1, "lo @ {lo:#x}");
            assert_eq!(mem.read32(hi), 2, "hi @ {hi:#x}");
        }
    }

    #[test]
    fn stm_with_base_in_list_writeback_rules() {
        let mut mem = Ram::new();
        // stmia r0!, {r0, r1} ; stmia r1!, {r0, r1}
        mem.load_arm(0x100, &[0xE8A0_0003, 0xE8A1_0003]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.regs.set(1, 0x2000);
        cpu.step(&mut mem);
        assert_eq!(
            mem.read32(0x1000),
            0x1000,
            "base first in list: original value"
        );
        assert_eq!(cpu.regs.get(0), 0x1008);
        cpu.step(&mut mem);
        assert_eq!(
            mem.read32(0x2004),
            0x2008,
            "base not first: written-back value"
        );
    }

    #[test]
    fn ldm_with_base_in_list_loads_over_writeback() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE8B0_0003]); // ldmia r0!, {r0, r1}
        mem.write32(0x1000, 0x55);
        mem.write32(0x1004, 0x66);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0x55);
        assert_eq!(cpu.regs.get(1), 0x66);
    }

    #[test]
    fn empty_list_quirk() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE8A0_0000]); // stmia r0!, {}
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.step(&mut mem);
        assert_eq!(mem.read32(0x1000), 0x10C);
        assert_eq!(cpu.regs.get(0), 0x1040);
    }

    #[test]
    fn s_bit_transfers_user_bank_and_restores_cpsr() {
        let mut mem = Ram::new();
        // stmia r0, {sp, lr}^ ; ldmia r0, {pc}^
        mem.load_arm(0x100, &[0xE8C0_6000, 0xE8D0_8000]);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(SP, 0xAAAA);
        cpu.regs.switch_mode(Mode::Irq);
        cpu.regs.set(SP, 0xBBBB);
        cpu.regs.set(0, 0x1000);
        cpu.regs.set_spsr(crate::cpu::Cpsr(0x0000_001F));
        cpu.step(&mut mem);
        assert_eq!(mem.read32(0x1000), 0xAAAA, "User-bank sp stored");
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.mode(), Mode::System);
        assert_eq!(cpu.next_pc(), 0xAAA8);
        let _ = PC;
    }

    #[test]
    fn swap_word_and_byte() {
        let mut mem = Ram::new();
        mem.load_arm(0x100, &[0xE100_0091, 0xE142_3091]); // swp r0, r1, [r0] ; swpb r3, r1, [r2]
        mem.write32(0x1000, 0xDEAD_BEEF);
        mem.write8(0x2001, 0x42);
        let mut cpu = arm_at(&mut mem, 0x100);
        cpu.regs.set(0, 0x1000);
        cpu.regs.set(1, 0x1122_3344);
        cpu.regs.set(2, 0x2001);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(0), 0xDEAD_BEEF);
        assert_eq!(mem.read32(0x1000), 0x1122_3344);
        cpu.step(&mut mem);
        assert_eq!(cpu.regs.get(3), 0x42);
        assert_eq!(mem.read8(0x2001), 0x44);
    }
}
