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

/// Minimal replacement for the BIOS's exception vectors, installed when
/// no BIOS image is loaded. The IRQ vector mirrors the real entry code:
/// save the caller-saved registers, jump to the handler the game placed
/// at `0x03007FFC`, restore and return.
///
/// ```text
/// 0x18: b     0x20
/// 0x20: stmfd sp!, {r0-r3, r12, lr}
/// 0x24: mov   r0, #0x04000000
/// 0x28: add   lr, pc, #0          ; lr = 0x30
/// 0x2C: ldr   pc, [r0, #-4]       ; [0x03FFFFFC] = [0x03007FFC]
/// 0x30: ldmfd sp!, {r0-r3, r12, lr}
/// 0x34: subs  pc, lr, #4
/// ```
pub const IRQ_STUB: [(u32, u32); 7] = [
    (0x18, 0xEA00_0000),
    (0x20, 0xE92D_500F),
    (0x24, 0xE3A0_0301),
    (0x28, 0xE28F_E000),
    (0x2C, 0xE510_F004),
    (0x30, 0xE8BD_500F),
    (0x34, 0xE25E_F004),
];

/// Address of the game's IRQ handler pointer, read by the BIOS.
pub const IRQ_HANDLER_POINTER: u32 = 0x0300_7FFC;

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
    pub const BIT_UNPACK: u8 = 0x10;
    pub const LZ77_UNCOMP_WRAM: u8 = 0x11;
    pub const LZ77_UNCOMP_VRAM: u8 = 0x12;
    pub const HUFF_UNCOMP: u8 = 0x13;
    pub const RL_UNCOMP_WRAM: u8 = 0x14;
    pub const RL_UNCOMP_VRAM: u8 = 0x15;
    pub const SOUND_BIAS: u8 = 0x19;
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
        call::HALT | call::STOP => {
            cpu.halted = true;
            Outcome::Done
        }
        call::SOUND_BIAS => Outcome::Done,
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
        call::RL_UNCOMP_WRAM => {
            rl_uncomp(mem, r(0), r(1), false);
            Outcome::Done
        }
        call::RL_UNCOMP_VRAM => {
            rl_uncomp(mem, r(0), r(1), true);
            Outcome::Done
        }
        call::BIT_UNPACK => {
            bit_unpack(mem, r(0), r(1), r(2));
            Outcome::Done
        }
        call::BG_AFFINE_SET => {
            bg_affine_set(mem, r(0), r(1), r(2));
            Outcome::Done
        }
        call::OBJ_AFFINE_SET => {
            obj_affine_set(mem, r(0), r(1), r(2), r(3));
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

/// Writes decompressed bytes to `dst`, as halfwords when `vram` is set
/// because VRAM ignores byte writes.
fn write_out(mem: &mut impl Memory, dst: u32, out: &[u8], vram: bool) {
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

/// `RLUnCompWram`/`RLUnCompVram`: run-length decoding. Each flag byte
/// either repeats the next byte `(flag & 0x7F) + 3` times (bit 7 set) or
/// copies the next `(flag & 0x7F) + 1` bytes literally.
fn rl_uncomp(mem: &mut impl Memory, src: u32, dst: u32, vram: bool) {
    let header = mem.read32(src);
    if header & 0xF0 != 0x30 {
        return;
    }
    let size = (header >> 8) as usize;
    let mut src = src + 4;
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let flag = mem.read8(src);
        src += 1;
        if flag & 0x80 != 0 {
            let byte = mem.read8(src);
            src += 1;
            out.extend(std::iter::repeat_n(byte, usize::from(flag & 0x7F) + 3));
        } else {
            for _ in 0..=usize::from(flag & 0x7F) {
                out.push(mem.read8(src));
                src += 1;
            }
        }
    }
    out.truncate(size);
    write_out(mem, dst, &out, vram);
}

/// `BitUnPack`: widens `src_bits`-wide units into `dst_bits`-wide units,
/// adding `offset` to each (non-zero, unless bit 31 of the offset word
/// is set) value, and writes the result as words.
fn bit_unpack(mem: &mut impl Memory, src: u32, dst: u32, info: u32) {
    let length = u32::from(mem.read16(info));
    let src_bits = u32::from(mem.read8(info + 2));
    let dst_bits = u32::from(mem.read8(info + 3));
    let offset_word = mem.read32(info + 4);
    let offset = offset_word & 0x7FFF_FFFF;
    let offset_zeros = offset_word & (1 << 31) != 0;
    if !matches!(src_bits, 1 | 2 | 4 | 8) || !matches!(dst_bits, 1 | 2 | 4 | 8 | 16 | 32) {
        return;
    }

    let mut out_word = 0u32;
    let mut out_bits = 0;
    let mut dst = dst;
    for i in 0..length {
        let byte = u32::from(mem.read8(src + i));
        for chunk in 0..8 / src_bits {
            let mut value = (byte >> (chunk * src_bits)) & ((1 << src_bits) - 1);
            if value != 0 || offset_zeros {
                value = value.wrapping_add(offset);
            }
            out_word |= value.checked_shl(out_bits).unwrap_or(0);
            out_bits += dst_bits;
            if out_bits >= 32 {
                mem.write32(dst, out_word);
                dst += 4;
                out_word = 0;
                out_bits = 0;
            }
        }
    }
}

/// `sin`/`cos` of a BIOS angle (`0..=0xFF` = one turn) in 8.8 fixed point.
fn sin_cos(theta: u16) -> (i32, i32) {
    let angle = f64::from(theta & 0xFF) / 256.0 * std::f64::consts::TAU;
    (
        (angle.sin() * 256.0).round() as i32,
        (angle.cos() * 256.0).round() as i32,
    )
}

/// The rotation/scaling matrix for scale `(sx, sy)` (8.8) and `theta`.
fn affine_matrix(sx: i32, sy: i32, theta: u16) -> [i32; 4] {
    let (sin, cos) = sin_cos(theta);
    [
        (sx * cos) >> 8,
        -(sx * sin) >> 8,
        (sy * sin) >> 8,
        (sy * cos) >> 8,
    ]
}

/// `BgAffineSet`: `count` sets of BG rotation parameters.
///
/// Source (20 bytes): texture origin x/y (24.8), screen centre x/y
/// (s16), scale x/y (8.8), angle (upper byte used).
/// Destination (16 bytes): PA–PD then the reference point DX/DY.
fn bg_affine_set(mem: &mut impl Memory, mut src: u32, mut dst: u32, count: u32) {
    for _ in 0..count {
        let ox = mem.read32(src) as i32;
        let oy = mem.read32(src + 4) as i32;
        let cx = i32::from(mem.read16(src + 8) as i16);
        let cy = i32::from(mem.read16(src + 10) as i16);
        let sx = i32::from(mem.read16(src + 12) as i16);
        let sy = i32::from(mem.read16(src + 14) as i16);
        let theta = mem.read16(src + 16) >> 8;
        src += 20;

        let [pa, pb, pc, pd] = affine_matrix(sx, sy, theta);
        let dx = ox.wrapping_sub(pa.wrapping_mul(cx).wrapping_add(pb.wrapping_mul(cy)));
        let dy = oy.wrapping_sub(pc.wrapping_mul(cx).wrapping_add(pd.wrapping_mul(cy)));
        for (i, v) in [pa, pb, pc, pd].iter().enumerate() {
            mem.write16(dst + i as u32 * 2, *v as u16);
        }
        mem.write32(dst + 8, dx as u32);
        mem.write32(dst + 12, dy as u32);
        dst += 16;
    }
}

/// `ObjAffineSet`: `count` OAM matrices, each parameter `stride` bytes
/// apart (2 for packed output, 8 to write straight into OAM).
fn obj_affine_set(mem: &mut impl Memory, mut src: u32, mut dst: u32, count: u32, stride: u32) {
    for _ in 0..count {
        let sx = i32::from(mem.read16(src) as i16);
        let sy = i32::from(mem.read16(src + 2) as i16);
        let theta = mem.read16(src + 4) >> 8;
        src += 8;
        for v in affine_matrix(sx, sy, theta) {
            mem.write16(dst, v as u16);
            dst += stride;
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
    fn rl_decompresses_runs_and_literals() {
        let mut mem = Ram::new();
        // header 0x30, size 7 ; run of 4 'A' ; literal "BCD"
        let data: [u8; 10] = [0x30, 7, 0, 0, 0x81, b'A', 0x02, b'B', b'C', b'D'];
        for (i, b) in data.iter().enumerate() {
            mem.write8(0x1000 + i as u32, *b);
        }
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2000)]);
        service(call::RL_UNCOMP_WRAM, &mut cpu, &mut mem);
        let out: Vec<u8> = (0..7).map(|i| mem.read8(0x2000 + i)).collect();
        assert_eq!(&out, b"AAAABCD");
    }

    #[test]
    fn bit_unpack_widens_units() {
        let mut mem = Ram::new();
        // Two bytes of 1-bit units -> 4-bit units with offset 5 for non-zero.
        mem.write8(0x1000, 0b0000_0101);
        mem.write8(0x1001, 0b0000_0001);
        mem.write16(0x1100, 2); // length
        mem.write8(0x1102, 1); // src bits
        mem.write8(0x1103, 4); // dst bits
        mem.write32(0x1104, 5); // offset
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2000), (2, 0x1100)]);
        service(call::BIT_UNPACK, &mut cpu, &mut mem);
        assert_eq!(
            mem.read32(0x2000),
            0x0000_0606,
            "bits 0 and 2 of byte 0, plus offset"
        );
        assert_eq!(mem.read32(0x2004), 0x0000_0006, "bit 0 of byte 1");
    }

    #[test]
    fn affine_set_identity_and_rotation() {
        let mut mem = Ram::new();
        // ObjAffineSet: scale 1.0, theta 0 -> identity; stride 8 (OAM layout).
        mem.write16(0x1000, 0x100);
        mem.write16(0x1002, 0x100);
        mem.write16(0x1004, 0);
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x2006), (2, 1), (3, 8)]);
        service(call::OBJ_AFFINE_SET, &mut cpu, &mut mem);
        assert_eq!(mem.read16(0x2006), 0x100);
        assert_eq!(mem.read16(0x200E), 0);
        assert_eq!(mem.read16(0x2016), 0);
        assert_eq!(mem.read16(0x201E), 0x100);

        // 90 degrees (theta 0x4000): pa = 0, pb = -1.0, pc = 1.0, pd = 0.
        mem.write16(0x1004, 0x4000);
        let mut cpu = cpu_with(&[(0, 0x1000), (1, 0x3000), (2, 1), (3, 2)]);
        service(call::OBJ_AFFINE_SET, &mut cpu, &mut mem);
        assert_eq!(mem.read16(0x3000), 0);
        assert_eq!(mem.read16(0x3002) as i16, -0x100);
        assert_eq!(mem.read16(0x3004), 0x100);
        assert_eq!(mem.read16(0x3006), 0);

        // BgAffineSet: origin (10, 20), centre (120, 80), identity.
        mem.write32(0x4000, 10 << 8);
        mem.write32(0x4004, 20 << 8);
        mem.write16(0x4008, 120);
        mem.write16(0x400A, 80);
        mem.write16(0x400C, 0x100);
        mem.write16(0x400E, 0x100);
        mem.write16(0x4010, 0);
        let mut cpu = cpu_with(&[(0, 0x4000), (1, 0x5000), (2, 1)]);
        service(call::BG_AFFINE_SET, &mut cpu, &mut mem);
        assert_eq!(mem.read16(0x5000), 0x100);
        assert_eq!(mem.read32(0x5008) as i32, (10 << 8) - 120 * 0x100);
        assert_eq!(mem.read32(0x500C) as i32, (20 << 8) - 80 * 0x100);
    }

    #[test]
    fn unknown_calls_are_reported() {
        let mut mem = Ram::new();
        let mut cpu = Cpu::new();
        assert_eq!(service(0x2A, &mut cpu, &mut mem), Outcome::Unsupported);
    }
}
