//! Arithmetic helpers shared by the ARM and THUMB executors: the barrel
//! shifter and flag-producing add/subtract.

/// Barrel shifter operation (ARM bits 6:5, THUMB format 1/4 opcodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftType {
    /// Logical shift left.
    Lsl,
    /// Logical shift right.
    Lsr,
    /// Arithmetic shift right.
    Asr,
    /// Rotate right.
    Ror,
}

impl ShiftType {
    /// Decodes a two-bit shift type field.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        match bits & 0b11 {
            0b00 => Self::Lsl,
            0b01 => Self::Lsr,
            0b10 => Self::Asr,
            _ => Self::Ror,
        }
    }
}

/// Shift `value` by an *immediate* amount. Returns `(result, carry_out)`.
///
/// An immediate amount of `0` is special-cased per the ARM ARM: `LSL #0`
/// is a no-op that passes the carry through, while `LSR #0`/`ASR #0` mean
/// a 32-bit shift and `ROR #0` is `RRX` (rotate through carry).
#[must_use]
pub const fn shift_imm(kind: ShiftType, value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    match (kind, amount) {
        (ShiftType::Lsl, 0) => (value, carry_in),
        (ShiftType::Lsr, 0) => (0, value >> 31 != 0),
        (ShiftType::Asr, 0) => (((value as i32) >> 31) as u32, value >> 31 != 0),
        (ShiftType::Ror, 0) => (((carry_in as u32) << 31) | (value >> 1), value & 1 != 0),
        _ => shift_nonzero(kind, value, amount),
    }
}

/// Shift `value` by a *register-specified* amount (low 8 bits of Rs).
///
/// Unlike the immediate form, an amount of `0` leaves the value and carry
/// untouched, and amounts of 32 and above are meaningful.
#[must_use]
pub const fn shift_reg(kind: ShiftType, value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    let amount = amount & 0xFF;
    if amount == 0 {
        return (value, carry_in);
    }
    match kind {
        ShiftType::Ror => {
            let rot = amount & 0x1F;
            if rot == 0 {
                (value, value >> 31 != 0)
            } else {
                shift_nonzero(kind, value, rot)
            }
        }
        _ if amount < 32 => shift_nonzero(kind, value, amount),
        ShiftType::Lsl => (0, amount == 32 && value & 1 != 0),
        ShiftType::Lsr => (0, amount == 32 && value >> 31 != 0),
        ShiftType::Asr => (((value as i32) >> 31) as u32, value >> 31 != 0),
    }
}

/// Shift by `1..=31` (or `1..=31` rotate); the common non-degenerate case.
const fn shift_nonzero(kind: ShiftType, value: u32, amount: u32) -> (u32, bool) {
    debug_assert!(amount >= 1 && amount <= 31);
    match kind {
        ShiftType::Lsl => (value << amount, (value >> (32 - amount)) & 1 != 0),
        ShiftType::Lsr => (value >> amount, (value >> (amount - 1)) & 1 != 0),
        ShiftType::Asr => (
            ((value as i32) >> amount) as u32,
            (value >> (amount - 1)) & 1 != 0,
        ),
        ShiftType::Ror => (value.rotate_right(amount), (value >> (amount - 1)) & 1 != 0),
    }
}

/// `a + b + carry_in`, returning `(result, carry_out, overflow)`.
///
/// Subtraction is `add_with_carry(a, !b, carry)`: `SUB` uses `carry = true`,
/// `SBC` uses the C flag. This gives the ARM's "C = NOT borrow" convention
/// for free.
#[must_use]
pub const fn add_with_carry(a: u32, b: u32, carry_in: bool) -> (u32, bool, bool) {
    let (r1, c1) = a.overflowing_add(b);
    let (result, c2) = r1.overflowing_add(carry_in as u32);
    // Signed overflow: operands agree in sign and result disagrees.
    let overflow = ((a ^ result) & (b ^ result)) >> 31 != 0;
    (result, c1 | c2, overflow)
}

/// Internal cycles of the multiplier: 1–4 depending on how many
/// significant bytes the multiplier operand has. For signed multiplies,
/// leading all-ones bytes are just as cheap as leading zeros.
#[must_use]
pub const fn multiply_cycles(rs: u32, signed: bool) -> u32 {
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
    use super::*;

    #[test]
    fn lsl_immediate() {
        assert_eq!(
            shift_imm(ShiftType::Lsl, 0x8000_0001, 0, false),
            (0x8000_0001, false)
        );
        assert_eq!(
            shift_imm(ShiftType::Lsl, 0x8000_0001, 1, false),
            (0x2, true)
        );
        assert_eq!(
            shift_imm(ShiftType::Lsl, 0x1, 31, false),
            (0x8000_0000, false)
        );
    }

    #[test]
    fn lsr_and_asr_zero_means_32() {
        assert_eq!(shift_imm(ShiftType::Lsr, 0x8000_0000, 0, false), (0, true));
        assert_eq!(
            shift_imm(ShiftType::Asr, 0x8000_0000, 0, false),
            (0xFFFF_FFFF, true)
        );
        assert_eq!(shift_imm(ShiftType::Asr, 0x4000_0000, 0, true), (0, false));
        assert_eq!(shift_imm(ShiftType::Lsr, 0xF0, 4, false), (0xF, false));
        assert_eq!(shift_imm(ShiftType::Lsr, 0xF8, 4, false), (0xF, true));
        assert_eq!(
            shift_imm(ShiftType::Asr, 0x8000_0000, 4, false),
            (0xF800_0000, false)
        );
    }

    #[test]
    fn ror_zero_is_rrx() {
        assert_eq!(shift_imm(ShiftType::Ror, 0x3, 0, true), (0x8000_0001, true));
        assert_eq!(shift_imm(ShiftType::Ror, 0x2, 0, false), (0x1, false));
        assert_eq!(
            shift_imm(ShiftType::Ror, 0x1, 1, false),
            (0x8000_0000, true)
        );
    }

    #[test]
    fn register_shift_edge_cases() {
        assert_eq!(shift_reg(ShiftType::Lsl, 0xFF, 0, true), (0xFF, true));
        assert_eq!(shift_reg(ShiftType::Lsl, 0x1, 32, false), (0, true));
        assert_eq!(shift_reg(ShiftType::Lsl, 0x1, 33, true), (0, false));
        assert_eq!(shift_reg(ShiftType::Lsr, 0x8000_0000, 32, false), (0, true));
        assert_eq!(
            shift_reg(ShiftType::Lsr, 0x8000_0000, 40, false),
            (0, false)
        );
        assert_eq!(
            shift_reg(ShiftType::Asr, 0x8000_0000, 40, false),
            (0xFFFF_FFFF, true)
        );
        assert_eq!(
            shift_reg(ShiftType::Ror, 0x8000_0001, 32, false),
            (0x8000_0001, true)
        );
        assert_eq!(
            shift_reg(ShiftType::Ror, 0x1, 33, false),
            (0x8000_0000, true)
        );
        assert_eq!(shift_reg(ShiftType::Ror, 0xF0, 0x104, false), (0xF, false));
    }

    #[test]
    fn add_flags() {
        assert_eq!(add_with_carry(1, 2, false), (3, false, false));
        assert_eq!(add_with_carry(0xFFFF_FFFF, 1, false), (0, true, false));
        assert_eq!(
            add_with_carry(0x7FFF_FFFF, 1, false),
            (0x8000_0000, false, true)
        );
        assert_eq!(
            add_with_carry(0x8000_0000, 0x8000_0000, false),
            (0, true, true)
        );
        assert_eq!(add_with_carry(0xFFFF_FFFF, 0, true), (0, true, false));
    }

    #[test]
    fn sub_via_add_not_b() {
        // 5 - 3 = 2, no borrow -> C set
        assert_eq!(add_with_carry(5, !3, true), (2, true, false));
        // 3 - 5 = -2, borrow -> C clear
        assert_eq!(add_with_carry(3, !5, true), (0xFFFF_FFFE, false, false));
        // 0x80000000 - 1 overflows
        assert_eq!(
            add_with_carry(0x8000_0000, !1, true),
            (0x7FFF_FFFF, true, true)
        );
        // SBC with borrow: 5 - 3 - 1 = 1
        assert_eq!(add_with_carry(5, !3, false), (1, true, false));
    }
}
