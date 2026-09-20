//! High-level emulation of the BIOS system calls.
//!
//! The GBA BIOS is copyrighted and cannot ship with the emulator. When no
//! image is supplied, `SWI` instructions are intercepted and the most
//! common calls are serviced here in software. Games that rely on obscure
//! calls (or on BIOS memory contents) still need the real image.

use crate::cpu::Cpu;
use crate::memory::Memory;
use crate::memory::io::{Interrupt, reg};

/// IWRAM word the BIOS uses to communicate serviced interrupts to
/// `IntrWait`: the game's IRQ handler ORs the flags it handled into it.
pub const INTR_CHECK_FLAGS: u32 = 0x0300_7FF8;

/// System call numbers.
#[allow(missing_docs)]
pub mod call {
    pub const SOFT_RESET: u8 = 0x00;
    pub const REGISTER_RAM_RESET: u8 = 0x01;
    pub const HALT: u8 = 0x02;
    pub const STOP: u8 = 0x03;
    pub const INTR_WAIT: u8 = 0x04;
    pub const VBLANK_INTR_WAIT: u8 = 0x05;
    pub const DIV: u8 = 0x06;
    pub const DIV_ARM: u8 = 0x07;
    pub const SQRT: u8 = 0x08;
    pub const ARCTAN: u8 = 0x09;
    pub const ARCTAN2: u8 = 0x0A;
    pub const CPU_SET: u8 = 0x0B;
    pub const CPU_FAST_SET: u8 = 0x0C;
    pub const GET_BIOS_CHECKSUM: u8 = 0x0D;
    pub const BG_AFFINE_SET: u8 = 0x0E;
    pub const OBJ_AFFINE_SET: u8 = 0x0F;
    pub const LZ77_UNCOMP_WRAM: u8 = 0x11;
    pub const LZ77_UNCOMP_VRAM: u8 = 0x12;
}

/// Outcome of servicing a call that the caller must act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The call completed; continue executing.
    Done,
    /// The CPU should halt until an interrupt matching the mask is
    /// flagged in [`INTR_CHECK_FLAGS`] (`IntrWait` semantics).
    WaitForInterrupt {
        /// `IF`-style bit mask to wait for.
        mask: u16,
    },
    /// The call is not emulated; the caller may log it.
    Unsupported,
}

/// Services BIOS call `number` with the CPU's current registers.
pub fn service(number: u8, cpu: &mut Cpu, mem: &mut impl Memory) -> Outcome {
    let r = |i: usize| cpu.regs.get(i);
    match number {
        call::REGISTER_RAM_RESET => {
            register_ram_reset(mem, r(0) as u8);
            Outcome::Done
        }
        call::HALT => {
            cpu.halted = true;
            Outcome::Done
        }
        call::INTR_WAIT | call::VBLANK_INTR_WAIT => {
            let (discard_old, mask) = if number == call::VBLANK_INTR_WAIT {
                (true, Interrupt::VBlank.mask())
            } else {
                (r(0) != 0, r(1) as u16)
            };
            if discard_old {
                let flags = mem.read16(INTR_CHECK_FLAGS);
                mem.write16(INTR_CHECK_FLAGS, flags & !mask);
            }
            // Wait calls require the interrupt to be enabled; the real
            // BIOS forces IME on.
            mem.write16(0x0400_0000 + reg::IME, 1);
            cpu.halted = true;
            Outcome::WaitForInterrupt { mask }
        }
        call::DIV | call::DIV_ARM => {
            let (num, den) = if number == call::DIV {
                (r(0), r(1))
            } else {
                (r(1), r(0))
            };
            let (num, den) = (num as i32, den as i32);
            let (quot, rem) = if den == 0 {
                // Hardware yields ±1 and the numerator; not a crash.
                (if num < 0 { -1 } else { 1 }, num)
            } else {
                (num.wrapping_div(den), num.wrapping_rem(den))
            };
            cpu.regs.set(0, quot as u32);
            cpu.regs.set(1, rem as u32);
            cpu.regs.set(3, quot.unsigned_abs());
            Outcome::Done
        }
        call::SQRT => {
            cpu.regs.set(0, r(0).isqrt());
            Outcome::Done
        }
        call::ARCTAN2 => {
            cpu.regs.set(0, arctan2(r(0) as u16, r(1) as u16));
            Outcome::Done
        }
        call::CPU_SET => {
            cpu_set(mem, r(0), r(1), r(2));
            Outcome::Done
        }
        call::CPU_FAST_SET => {
            cpu_fast_set(mem, r(0), r(1), r(2));
            Outcome::Done
        }
        call::GET_BIOS_CHECKSUM => {
            cpu.regs.set(0, 0xBAAE_187F);
            Outcome::Done
        }
        call::LZ77_UNCOMP_WRAM => {
            lz77_uncomp(mem, r(0), r(1), false);
            Outcome::Done
        }
        call::LZ77_UNCOMP_VRAM => {
            lz77_uncomp(mem, r(0), r(1), true);
            Outcome::Done
        }
        _ => Outcome::Unsupported,
    }
}

/// `RegisterRamReset`: clears the memories selected by `flags` and resets
/// most I/O registers.
fn register_ram_reset(mem: &mut impl Memory, flags: u8) {
    use crate::memory::{EWRAM_SIZE, IWRAM_SIZE, OAM_SIZE, PALETTE_SIZE, VRAM_SIZE, base};
    let clear = |mem: &mut dyn FnMut(u32, u32), base: u32, size: usize| {
        for offset in (0..size as u32).step_by(4) {
            mem(base + offset, 0);
        }
    };
    let mut write = |address, value| mem.write32(address, value);
    if flags & 0x01 != 0 {
        clear(&mut write, base::EWRAM, EWRAM_SIZE);
    }
    if flags & 0x02 != 0 {
        // Everything but the top 0x200 bytes, which hold the BIOS's own state.
        clear(&mut write, base::IWRAM, IWRAM_SIZE - 0x200);
    }
    if flags & 0x04 != 0 {
        clear(&mut write, base::PALETTE, PALETTE_SIZE);
    }
    if flags & 0x08 != 0 {
        clear(&mut write, base::VRAM, VRAM_SIZE);
    }
    if flags & 0x10 != 0 {
        clear(&mut write, base::OAM, OAM_SIZE);
    }
    // Bits 5–7 reset the serial, sound and other I/O registers. We only
    // model the visible side effect games rely on: the display is blanked.
    if flags & 0x80 != 0 {
        mem.write16(base::IO + reg::DISPCNT, 0x0080);
    }
}

/// `LZ77UnCompWram`/`LZ77UnCompVram`: decompress the GBA's LZ77 format.
///
/// The header word holds the type (`0x10`) in its low byte and the
/// decompressed size in the upper 24 bits. Each block starts with a flag
/// byte whose bits (MSB first) mark the next eight tokens as literal
/// bytes (0) or 2-byte back-references (1): `LLLL DDDD DDDD DDDD` with
/// length `L + 3` and distance `D + 1`. The VRAM variant writes halfwords
/// because VRAM ignores byte writes.
fn lz77_uncomp(mem: &mut impl Memory, src: u32, dst: u32, vram: bool) {
    let header = mem.read32(src);
    if header & 0xFF != 0x10 {
        return;
    }
    let size = header >> 8;
    let mut src = src + 4;
    let mut out = Vec::with_capacity(size as usize);

    while (out.len() as u32) < size {
        let flags = mem.read8(src);
        src += 1;
        for bit in (0..8).rev() {
            if (out.len() as u32) >= size {
                break;
            }
            if flags & (1 << bit) == 0 {
                out.push(mem.read8(src));
                src += 1;
            } else {
                let b0 = mem.read8(src);
                let b1 = mem.read8(src + 1);
                src += 2;
                let length = usize::from(b0 >> 4) + 3;
                let distance = (usize::from(b0 & 0xF) << 8 | usize::from(b1)) + 1;
                for _ in 0..length {
                    let byte = out
                        .get(out.len().wrapping_sub(distance))
                        .copied()
                        .unwrap_or(0);
                    out.push(byte);
                }
            }
        }
    }

    if vram {
        for (i, pair) in out.chunks(2).enumerate() {
            let half = u16::from(pair[0]) | (u16::from(*pair.get(1).unwrap_or(&0)) << 8);
            mem.write16(dst + i as u32 * 2, half);
        }
    } else {
        for (i, &byte) in out.iter().enumerate() {
            mem.write8(dst + i as u32, byte);
        }
    }
}

/// `CpuSet`: copy or fill `count` halfwords/words. Bit 24 of `control`
/// selects fill (source is read once), bit 26 selects 32-bit units.
fn cpu_set(mem: &mut impl Memory, src: u32, dst: u32, control: u32) {
    let count = control & 0x1F_FFFF;
    let fill = control & (1 << 24) != 0;
    if control & (1 << 26) != 0 {
        let (src, dst) = (src & !3, dst & !3);
        let fill_value = mem.read32(src);
        for i in 0..count {
            let value = if fill {
                fill_value
            } else {
                mem.read32(src + i * 4)
            };
            mem.write32(dst + i * 4, value);
        }
    } else {
        let (src, dst) = (src & !1, dst & !1);
        let fill_value = mem.read16(src);
        for i in 0..count {
            let value = if fill {
                fill_value
            } else {
                mem.read16(src + i * 2)
            };
            mem.write16(dst + i * 2, value);
        }
    }
}

/// `CpuFastSet`: like the 32-bit `CpuSet` but in 8-word blocks; a count
/// that is not a multiple of 8 is rounded up.
fn cpu_fast_set(mem: &mut impl Memory, src: u32, dst: u32, control: u32) {
    let count = (control & 0x1F_FFFF).div_ceil(8) * 8;
    cpu_set(mem, src, dst, count | (control & (1 << 24)) | (1 << 26));
}

/// `ArcTan2(x, y)`: angle of `(x, y)` as a 16-bit fraction of a turn.
fn arctan2(x: u16, y: u16) -> u32 {
    let (x, y) = (f64::from(x as i16), f64::from(y as i16));
    let angle = y.atan2(x) / std::f64::consts::TAU;
    ((angle * 65536.0).round() as i32 as u32) & 0xFFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::test_util::Ram;

    fn cpu_with(regs: &[(usize, u32)]) -> Cpu {
        let mut cpu = Cpu::new();
        for &(i, v) in regs {
            cpu.regs.set(i, v);
        }
        cpu
    }

    #[test]
    fn div_and_div_arm() {
        let mut mem = Ram::new();
        let mut cpu = cpu_with(&[(0, (-7i32) as u32), (1, 2)]);
        assert_eq!(service(call::DIV, &mut cpu, &mut mem), Outcome::Done);
        assert_eq!(cpu.regs.get(0), (-3i32) as u32);
        assert_eq!(cpu.regs.get(1), (-1i32) as u32);
        assert_eq!(cpu.regs.get(3), 3);

        let mut cpu = cpu_with(&[(0, 2), (1, 7)]);
        service(call::DIV_ARM, &mut cpu, &mut mem);
        assert_eq!(cpu.regs.get(0), 3);

        let mut cpu = cpu_with(&[(0, 9), (1, 0)]);
        service(call::DIV, &mut cpu, &mut mem);
        assert_eq!((cpu.regs.get(0), cpu.regs.get(1)), (1, 9), "divide by zero");
    }

    #[test]
    fn sqrt_and_arctan2() {
        let mut mem = Ram::new();
        let mut cpu = cpu_with(&[(0, 1_000_000)]);
        service(call::SQRT, &mut cpu, &mut mem);
        assert_eq!(cpu.regs.get(0), 1000);

        let mut cpu = cpu_with(&[(0, 0), (1, 0x100)]); // straight up
        service(call::ARCTAN2, &mut cpu, &mut mem);
        assert_eq!(cpu.regs.get(0), 0x4000);
        let mut cpu = cpu_with(&[(0, 0xFF00), (1, 0)]); // x = -0x100: left
        service(call::ARCTAN2, &mut cpu, &mut mem);
        assert_eq!(cpu.regs.get(0), 0x8000);
    }

    #[test]
    fn cpu_set_copies_and_fills() {
        let mut mem = Ram::new();
        mem.write32(0x1000, 0xAABB_CCDD);
        mem.write32(0x1004, 0x1122_3344);
        // 16-bit copy of 4 halfwords
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2000), (2, 4)]);
        service(call::CPU_SET, &mut cpu, &mut mem);
        assert_eq!(mem.read32(0x2000), 0xAABB_CCDD);
        assert_eq!(mem.read32(0x2004), 0x1122_3344);
        // 32-bit fill of 3 words
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x3000), (2, 3 | (1 << 24) | (1 << 26))]);
        service(call::CPU_SET, &mut cpu, &mut mem);
        assert_eq!(mem.read32(0x3000), 0xAABB_CCDD);
        assert_eq!(mem.read32(0x3008), 0xAABB_CCDD);
        assert_eq!(mem.read32(0x300C), 0);
    }

    #[test]
    fn cpu_fast_set_rounds_to_blocks() {
        let mut mem = Ram::new();
        mem.write32(0x1000, 0x5555_5555);
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2000), (2, 9 | (1 << 24))]);
        service(call::CPU_FAST_SET, &mut cpu, &mut mem);
        assert_eq!(
            mem.read32(0x2000 + 15 * 4),
            0x5555_5555,
            "rounded up to 16 words"
        );
        assert_eq!(mem.read32(0x2000 + 16 * 4), 0);
    }

    #[test]
    fn vblank_intr_wait_halts_and_clears_flag() {
        let mut mem = Ram::new();
        mem.write16(INTR_CHECK_FLAGS, 0b11);
        let mut cpu = Cpu::new();
        let out = service(call::VBLANK_INTR_WAIT, &mut cpu, &mut mem);
        assert_eq!(out, Outcome::WaitForInterrupt { mask: 1 });
        assert!(cpu.halted);
        assert_eq!(mem.read16(INTR_CHECK_FLAGS), 0b10);
        assert_eq!(mem.read16(0x0400_0208), 1, "IME forced on");
    }

    #[test]
    fn intr_wait_keeps_old_flags_when_r0_is_zero() {
        let mut mem = Ram::new();
        mem.write16(INTR_CHECK_FLAGS, 0b1000);
        let mut cpu = cpu_with(&[(0, 0), (1, 0b1000)]);
        assert_eq!(
            service(call::INTR_WAIT, &mut cpu, &mut mem),
            Outcome::WaitForInterrupt { mask: 0b1000 }
        );
        assert_eq!(mem.read16(INTR_CHECK_FLAGS), 0b1000);
    }

    #[test]
    fn register_ram_reset_clears_selected_memories() {
        use crate::memory::test_util::rom_with_header;
        use crate::memory::{Bus, Cartridge, base};
        let mut bus = Bus::new(Cartridge::from_bytes(rom_with_header("R", 0x100)).unwrap());
        bus.write32(base::EWRAM, 1);
        bus.write32(base::IWRAM, 2);
        bus.write32(base::IWRAM + 0x7FF0, 3);
        bus.write32(base::VRAM, 4);
        let mut cpu = cpu_with(&[(0, 0x01 | 0x02 | 0x80)]);
        service(call::REGISTER_RAM_RESET, &mut cpu, &mut bus);
        assert_eq!(bus.read32(base::EWRAM), 0);
        assert_eq!(bus.read32(base::IWRAM), 0);
        assert_eq!(
            bus.read32(base::IWRAM + 0x7FF0),
            3,
            "top of IWRAM preserved"
        );
        assert_eq!(bus.read32(base::VRAM), 4, "VRAM not selected");
        assert_eq!(bus.io.read16(reg::DISPCNT), 0x0080, "display blanked");
    }

    #[test]
    fn lz77_decompresses_literals_and_references() {
        let mut mem = Ram::new();
        // "ABCABCABCD": literals A B C, then a 7-byte back-reference of
        // distance 3, then literal D.  Header: type 0x10, size 11.
        let compressed: [u8; 12] = [
            0x10,
            11,
            0,
            0, // header
            0b0001_0000,
            b'A',
            b'B',
            b'C',
            0x40,
            0x02,
            b'D', // flags, tokens
            0x00, // padding
        ];
        // token 4 is a reference: 0x40 -> length 4+3 = 7, distance 2+1 = 3
        for (i, b) in compressed.iter().enumerate() {
            mem.write8(0x1000 + i as u32, *b);
        }
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2000)]);
        service(call::LZ77_UNCOMP_WRAM, &mut cpu, &mut mem);
        let out: Vec<u8> = (0..11).map(|i| mem.read8(0x2000 + i)).collect();
        assert_eq!(&out, b"ABCABCABCAD");

        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x3000)]);
        service(call::LZ77_UNCOMP_VRAM, &mut cpu, &mut mem);
        assert_eq!(mem.read16(0x3000), u16::from_le_bytes(*b"AB"));
        assert_eq!(mem.read8(0x300A), b'D');
    }

    #[test]
    fn unknown_calls_are_reported() {
        let mut mem = Ram::new();
        let mut cpu = Cpu::new();
        assert_eq!(service(0x2A, &mut cpu, &mut mem), Outcome::Unsupported);
    }
}
