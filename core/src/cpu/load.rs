//! Load helpers implementing the ARM7TDMI's misaligned-access behaviour,
//! shared by the ARM and THUMB executors.
//!
//! The core never faults on misalignment. Instead a misaligned word load
//! fetches the aligned word and rotates it so the addressed byte ends up in
//! the low bits; halfword loads behave analogously, and `LDRSH` from an odd
//! address degrades to a sign-extended byte load.

use crate::memory::Memory;

/// `LDR`: word load with rotation for misaligned addresses.
#[inline]
pub fn word(mem: &impl Memory, address: u32) -> u32 {
    mem.read32(address & !3).rotate_right((address & 3) * 8)
}

/// `LDRH`: halfword load, zero-extended, rotated when misaligned.
#[inline]
pub fn halfword(mem: &impl Memory, address: u32) -> u32 {
    u32::from(mem.read16(address & !1)).rotate_right((address & 1) * 8)
}

/// `LDRSH`: sign-extended halfword; misaligned addresses load a signed byte.
#[inline]
pub fn signed_halfword(mem: &impl Memory, address: u32) -> u32 {
    if address & 1 == 0 {
        i32::from(mem.read16(address) as i16) as u32
    } else {
        signed_byte(mem, address)
    }
}

/// `LDRSB`: sign-extended byte.
#[inline]
pub fn signed_byte(mem: &impl Memory, address: u32) -> u32 {
    i32::from(mem.read8(address) as i8) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::test_util::Ram;
    use crate::memory::Memory;

    fn ram() -> Ram {
        let mut ram = Ram::new();
        ram.write32(0x100, 0x8877_6655);
        ram
    }

    #[test]
    fn misaligned_word_rotates() {
        let ram = ram();
        assert_eq!(word(&ram, 0x100), 0x8877_6655);
        assert_eq!(word(&ram, 0x101), 0x5588_7766);
        assert_eq!(word(&ram, 0x102), 0x6655_8877);
        assert_eq!(word(&ram, 0x103), 0x7766_5588);
    }

    #[test]
    fn misaligned_halfword_rotates() {
        let ram = ram();
        assert_eq!(halfword(&ram, 0x100), 0x6655);
        assert_eq!(halfword(&ram, 0x101), 0x5500_0066);
    }

    #[test]
    fn signed_loads_extend() {
        let ram = ram();
        assert_eq!(signed_byte(&ram, 0x100), 0x55);
        assert_eq!(signed_byte(&ram, 0x103), 0xFFFF_FF88);
        assert_eq!(signed_halfword(&ram, 0x102), 0xFFFF_8877);
        assert_eq!(
            signed_halfword(&ram, 0x103),
            0xFFFF_FF88,
            "odd address -> LDRSB"
        );
    }
}
