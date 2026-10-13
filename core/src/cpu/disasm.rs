//! ARM and THUMB disassembly, for debugging output.
//!
//! The syntax is unified assembler language (UAL) in lower case, the way
//! `objdump` prints it: flag-setting and condition suffixes follow the
//! mnemonic (`addseq`, `ldrbne`), `stmdb sp!`/`ldmia sp!` read as
//! `push`/`pop`, and PC-relative operands end with a comment giving the
//! address they resolve to.

use std::fmt::Write;

use crate::cpu::arm::{ArmKind, decode as arm_decode};

const CONDITIONS: [&str; 16] = [
    "eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "", "nv",
];

const SHIFTS: [&str; 4] = ["lsl", "lsr", "asr", "ror"];

/// The name of register `n`, with the stack pointer, link register and
/// program counter by their roles.
fn reg(n: u32) -> &'static str {
    const NAMES: [&str; 16] = [
        "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "sp",
        "lr", "pc",
    ];
    NAMES[(n & 0xF) as usize]
}

/// Formats an immediate the way people read them: small values in
/// decimal, the rest in hex.
fn imm(value: u32) -> String {
    if value < 10 {
        format!("#{value}")
    } else {
        format!("#{value:#x}")
    }
}

/// A signed immediate offset, `#-0x10` style.
fn signed_imm(up: bool, value: u32) -> String {
    if up {
        imm(value)
    } else {
        format!("#-{}", &imm(value)[1..])
    }
}

/// `{r0-r3, lr}` for a register-list bit mask.
fn reg_list(mask: u32) -> String {
    let mut out = String::from("{");
    let mut n = 0;
    while n < 16 {
        if mask & (1 << n) == 0 {
            n += 1;
            continue;
        }
        let start = n;
        // sp, lr and pc are named, so they never join a range.
        while n < 16 && mask & (1 << n) != 0 && (n == start || n < 13) {
            n += 1;
        }
        if out.len() > 1 {
            out.push_str(", ");
        }
        out.push_str(reg(start));
        // Runs of three or more collapse into a range.
        match n - start {
            1 => {}
            2 => write!(out, ", {}", reg(start + 1)).unwrap(),
            _ => write!(out, "-{}", reg(n - 1)).unwrap(),
        }
    }
    out.push('}');
    out
}

/// Sign-extends the low `bits` bits of `value`.
fn sign_extend(value: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((value << shift) as i32) >> shift
}

/// Disassembles the ARM instruction `word` found at `address`.
///
/// The instruction class comes from the CPU's own decoder, so the text
/// says what the emulator will execute even where the encoding breaks a
/// should-be-zero rule that other disassemblers reject.
#[must_use]
pub fn arm(address: u32, word: u32) -> String {
    let cond = CONDITIONS[(word >> 28) as usize];
    let rn = (word >> 16) & 0xF;
    let rd = (word >> 12) & 0xF;
    let rs = (word >> 8) & 0xF;
    let rm = word & 0xF;
    let bit = |n: u32| word & (1 << n) != 0;

    match arm_decode(word) {
        ArmKind::BranchExchange => format!("bx{cond} {}", reg(rm)),
        ArmKind::Multiply => {
            let s = if bit(20) { "s" } else { "" };
            if bit(21) {
                format!(
                    "mla{s}{cond} {}, {}, {}, {}",
                    reg(rn),
                    reg(rm),
                    reg(rs),
                    reg(rd)
                )
            } else {
                format!("mul{s}{cond} {}, {}, {}", reg(rn), reg(rm), reg(rs))
            }
        }
        ArmKind::MultiplyLong => {
            let sign = if bit(22) { "s" } else { "u" };
            let op = if bit(21) { "mlal" } else { "mull" };
            let s = if bit(20) { "s" } else { "" };
            format!(
                "{sign}{op}{s}{cond} {}, {}, {}, {}",
                reg(rd),
                reg(rn),
                reg(rm),
                reg(rs)
            )
        }
        ArmKind::SingleDataSwap => {
            let b = if bit(22) { "b" } else { "" };
            format!("swp{b}{cond} {}, {}, [{}]", reg(rd), reg(rm), reg(rn))
        }
        ArmKind::HalfwordTransfer => arm_halfword(address, word, cond),
        ArmKind::PsrTransfer => {
            let psr = if bit(22) { "spsr" } else { "cpsr" };
            if !bit(21) {
                return format!("mrs{cond} {}, {psr}", reg(rd));
            }
            let mut fields = String::new();
            for (n, name) in [(19, 'f'), (18, 's'), (17, 'x'), (16, 'c')] {
                if bit(n) {
                    fields.push(name);
                }
            }
            let source = if bit(25) {
                imm((word & 0xFF).rotate_right(2 * ((word >> 8) & 0xF)))
            } else {
                reg(rm).to_string()
            };
            format!("msr{cond} {psr}_{fields}, {source}")
        }
        ArmKind::DataProcessing => arm_data_processing(word, cond),
        ArmKind::SingleDataTransfer => arm_single_transfer(address, word, cond),
        ArmKind::BlockDataTransfer => arm_block_transfer(word, cond),
        ArmKind::Branch => {
            let link = if bit(24) { "l" } else { "" };
            let target = address
                .wrapping_add(8)
                .wrapping_add_signed(sign_extend(word & 0xFF_FFFF, 24) << 2);
            format!("b{link}{cond} {target:#010x}")
        }
        ArmKind::SoftwareInterrupt => format!("swi{cond} {}", imm(word & 0xFF_FFFF)),
        ArmKind::Coprocessor | ArmKind::Undefined => "undefined".into(),
    }
}

/// The second operand of a data-processing instruction.
fn arm_operand2(word: u32) -> String {
    if word & (1 << 25) != 0 {
        return imm((word & 0xFF).rotate_right(2 * ((word >> 8) & 0xF)));
    }
    let rm = reg(word & 0xF);
    let kind = (word >> 5) & 0b11;
    if word & (1 << 4) != 0 {
        return format!("{rm}, {} {}", SHIFTS[kind as usize], reg((word >> 8) & 0xF));
    }
    shifted_register(rm, kind, (word >> 7) & 0x1F)
}

/// `rm` shifted by an immediate, with the encodings for 32 and `rrx`.
fn shifted_register(rm: &str, kind: u32, amount: u32) -> String {
    match (kind, amount) {
        (0, 0) => rm.to_string(),
        (3, 0) => format!("{rm}, rrx"),
        (1 | 2, 0) => format!("{rm}, {} #32", SHIFTS[kind as usize]),
        _ => format!("{rm}, {} #{amount}", SHIFTS[kind as usize]),
    }
}

fn arm_data_processing(word: u32, cond: &str) -> String {
    const OPS: [&str; 16] = [
        "and", "eor", "sub", "rsb", "add", "adc", "sbc", "rsc", "tst", "teq", "cmp", "cmn", "orr",
        "mov", "bic", "mvn",
    ];
    let op = (word >> 21) & 0xF;
    let rn = reg((word >> 16) & 0xF);
    let rd = reg((word >> 12) & 0xF);
    let operand = arm_operand2(word);
    let name = OPS[op as usize];
    // The comparisons always set the flags; no `s`.
    if (8..=11).contains(&op) {
        return format!("{name}{cond} {rn}, {operand}");
    }
    let s = if word & (1 << 20) != 0 { "s" } else { "" };
    if op == 13 || op == 15 {
        format!("{name}{s}{cond} {rd}, {operand}")
    } else {
        format!("{name}{s}{cond} {rd}, {rn}, {operand}")
    }
}

/// `[rn, offset]`, `[rn, offset]!` or `[rn], offset` for a load or store,
/// plus a comment with the address when it is PC-relative.
fn address_mode(
    address: u32,
    rn: u32,
    pre: bool,
    writeback: bool,
    offset: Option<(bool, u32)>,
    register: Option<String>,
) -> String {
    let base = reg(rn);
    let offset_text = register.or_else(|| match offset {
        Some((_, 0)) | None => None,
        Some((up, value)) => Some(signed_imm(up, value)),
    });
    let mut out = match (pre, offset_text) {
        (true, None) => format!("[{base}]"),
        (true, Some(o)) => format!("[{base}, {o}]"),
        (false, None) => format!("[{base}], #0"),
        (false, Some(o)) => format!("[{base}], {o}"),
    };
    if pre && writeback {
        out.push('!');
    }
    if rn == 15
        && pre
        && let Some((up, value)) = offset
    {
        let pc = address.wrapping_add(8);
        let target = if up {
            pc.wrapping_add(value)
        } else {
            pc.wrapping_sub(value)
        };
        write!(out, " ; {target:#010x}").unwrap();
    }
    out
}

fn arm_single_transfer(address: u32, word: u32, cond: &str) -> String {
    let bit = |n: u32| word & (1 << n) != 0;
    let (pre, up) = (bit(24), bit(23));
    let name = if bit(20) { "ldr" } else { "str" };
    let b = if bit(22) { "b" } else { "" };
    // Post-indexed with W set is the user-mode ("translated") access.
    let t = if !pre && bit(21) { "t" } else { "" };
    let rn = (word >> 16) & 0xF;
    let rd = reg((word >> 12) & 0xF);
    let (offset, register) = if bit(25) {
        let sign = if up { "" } else { "-" };
        let shifted = shifted_register(reg(word & 0xF), (word >> 5) & 0b11, (word >> 7) & 0x1F);
        (None, Some(format!("{sign}{shifted}")))
    } else {
        (Some((up, word & 0xFFF)), None)
    };
    let mode = address_mode(address, rn, pre, bit(21), offset, register);
    format!("{name}{b}{t}{cond} {rd}, {mode}")
}

fn arm_halfword(address: u32, word: u32, cond: &str) -> String {
    let bit = |n: u32| word & (1 << n) != 0;
    let (pre, up) = (bit(24), bit(23));
    let name = match (bit(20), (word >> 5) & 0b11) {
        (false, 1) => "strh",
        // Signed stores are ARMv5's doubleword transfers.
        (false, _) => return "undefined".into(),
        (true, 1) => "ldrh",
        (true, 2) => "ldrsb",
        _ => "ldrsh",
    };
    let rn = (word >> 16) & 0xF;
    let rd = reg((word >> 12) & 0xF);
    let (offset, register) = if bit(22) {
        (Some((up, ((word >> 4) & 0xF0) | (word & 0xF))), None)
    } else {
        let sign = if up { "" } else { "-" };
        (None, Some(format!("{sign}{}", reg(word & 0xF))))
    };
    let mode = address_mode(address, rn, pre, bit(21), offset, register);
    format!("{name}{cond} {rd}, {mode}")
}

fn arm_block_transfer(word: u32, cond: &str) -> String {
    let bit = |n: u32| word & (1 << n) != 0;
    let load = bit(20);
    let rn = (word >> 16) & 0xF;
    let list = reg_list(word & 0xFFFF);
    let caret = if bit(22) { "^" } else { "" };
    let (pre, up, writeback) = (bit(24), bit(23), bit(21));
    if rn == 13 && writeback && caret.is_empty() {
        if load && !pre && up {
            return format!("pop{cond} {list}");
        }
        if !load && pre && !up {
            return format!("push{cond} {list}");
        }
    }
    let name = if load { "ldm" } else { "stm" };
    let mode = match (pre, up) {
        // Increment-after is the default and goes unwritten.
        (false, true) => "",
        (true, true) => "ib",
        (false, false) => "da",
        (true, false) => "db",
    };
    let bang = if writeback { "!" } else { "" };
    format!("{name}{mode}{cond} {}{bang}, {list}{caret}", reg(rn))
}

/// Disassembles the THUMB instruction `half` found at `address`; `next`
/// is the halfword after it, which a `bl` pair needs. Returns the text
/// and how many bytes the instruction took (4 for a whole `bl` pair).
#[must_use]
pub fn thumb(address: u32, half: u16, next: u16) -> (String, u32) {
    let h = u32::from(half);
    let pc = address.wrapping_add(4);
    let low = |shift: u32| reg((h >> shift) & 7);
    let text = match h >> 11 {
        0b00000..=0b01011 => thumb_registers(h, pc),
        0b01100..=0b01111 => {
            let byte = h & (1 << 12) != 0;
            let load = h & (1 << 11) != 0;
            let op = ["str", "ldr", "strb", "ldrb"][usize::from(byte) * 2 + usize::from(load)];
            let offset = ((h >> 6) & 0x1F) * if byte { 1 } else { 4 };
            thumb_offset(op, low(0), low(3), offset)
        }
        0b10000 | 0b10001 => {
            let op = if h & (1 << 11) != 0 { "ldrh" } else { "strh" };
            thumb_offset(op, low(0), low(3), ((h >> 6) & 0x1F) * 2)
        }
        0b10010 | 0b10011 => {
            let op = if h & (1 << 11) != 0 { "ldr" } else { "str" };
            thumb_offset(op, low(8), "sp", (h & 0xFF) * 4)
        }
        0b10100 => {
            let offset = (h & 0xFF) * 4;
            format!(
                "add {}, pc, {} ; {:#010x}",
                low(8),
                imm(offset),
                (pc & !2).wrapping_add(offset)
            )
        }
        0b10101 => format!("add {}, sp, {}", low(8), imm((h & 0xFF) * 4)),
        0b10110 | 0b10111 => match (h >> 8) & 0xF {
            0b0000 => {
                let op = if h & 0x80 != 0 { "sub" } else { "add" };
                format!("{op} sp, {}", imm((h & 0x7F) * 4))
            }
            0b0100 | 0b0101 => format!("push {}", reg_list((h & 0xFF) | ((h >> 8) & 1) << 14)),
            0b1100 | 0b1101 => format!("pop {}", reg_list((h & 0xFF) | ((h >> 8) & 1) << 15)),
            _ => "undefined".into(),
        },
        0b11000 => format!("stmia {}!, {}", low(8), reg_list(h & 0xFF)),
        // A base in the list is overwritten by the load, not written back.
        0b11001 if h & (1 << ((h >> 8) & 7)) != 0 => {
            format!("ldmia {}, {}", low(8), reg_list(h & 0xFF))
        }
        0b11001 => format!("ldmia {}!, {}", low(8), reg_list(h & 0xFF)),
        0b11010 | 0b11011 => match (h >> 8) & 0xF {
            0xE => "undefined".into(),
            0xF => format!("swi {}", imm(h & 0xFF)),
            cond => {
                let target = pc.wrapping_add_signed(sign_extend(h & 0xFF, 8) << 1);
                format!("b{} {target:#010x}", CONDITIONS[cond as usize])
            }
        },
        0b11100 => {
            let target = pc.wrapping_add_signed(sign_extend(h & 0x7FF, 11) << 1);
            format!("b {target:#010x}")
        }
        0b11110 if next >> 11 == 0b11111 => {
            let high = sign_extend(h & 0x7FF, 11) << 12;
            let low = i32::from(next & 0x7FF) << 1;
            let target = pc.wrapping_add_signed(high + low);
            return (format!("bl {target:#010x}"), 4);
        }
        // A lone half of a `bl` pair: the first sets up lr, the second
        // jumps relative to it.
        0b11110 => format!("bl.hi {}", imm((h & 0x7FF) << 12)),
        0b11111 => format!("bl.lo lr, {}", imm((h & 0x7FF) << 1)),
        _ => "undefined".into(),
    };
    (text, 2)
}

/// THUMB formats 1 to 8: shifts, arithmetic, high registers and
/// register-offset loads and stores.
fn thumb_registers(h: u32, pc: u32) -> String {
    let low = |shift: u32| reg((h >> shift) & 7);
    match h >> 11 {
        // Shift by immediate; 00011 is add/sub.
        0b00000..=0b00010 => {
            let op = ["lsl", "lsr", "asr"][(h >> 11) as usize];
            let mut amount = (h >> 6) & 0x1F;
            if amount == 0 && op == "lsl" {
                return format!("movs {}, {}", low(0), low(3));
            }
            if amount == 0 && op != "lsl" {
                amount = 32;
            }
            format!("{op}s {}, {}, #{amount}", low(0), low(3))
        }
        0b00011 => {
            let op = if h & (1 << 9) != 0 { "sub" } else { "add" };
            let operand = if h & (1 << 10) != 0 {
                imm((h >> 6) & 7)
            } else {
                low(6).to_string()
            };
            format!("{op}s {}, {}, {operand}", low(0), low(3))
        }
        0b00100..=0b00111 => {
            let op = ["movs", "cmp", "adds", "subs"][((h >> 11) & 3) as usize];
            format!("{op} {}, {}", low(8), imm(h & 0xFF))
        }
        0b01000 if h & (1 << 10) == 0 => {
            const OPS: [&str; 16] = [
                "ands", "eors", "lsls", "lsrs", "asrs", "adcs", "sbcs", "rors", "tst", "negs",
                "cmp", "cmn", "orrs", "muls", "bics", "mvns",
            ];
            format!("{} {}, {}", OPS[((h >> 6) & 0xF) as usize], low(0), low(3))
        }
        0b01000 => {
            let rd = (h & 7) | ((h >> 4) & 8);
            let rs = (h >> 3) & 0xF;
            match (h >> 8) & 3 {
                0 => format!("add {}, {}", reg(rd), reg(rs)),
                1 => format!("cmp {}, {}", reg(rd), reg(rs)),
                2 => format!("mov {}, {}", reg(rd), reg(rs)),
                _ => format!("bx {}", reg(rs)),
            }
        }
        0b01001 => {
            let offset = (h & 0xFF) * 4;
            let target = (pc & !2).wrapping_add(offset);
            format!("ldr {}, [pc, {}] ; {target:#010x}", low(8), imm(offset))
        }
        0b01010 | 0b01011 => {
            let op = if h & (1 << 9) == 0 {
                ["str", "strb", "ldr", "ldrb"][((h >> 10) & 3) as usize]
            } else {
                ["strh", "ldrsb", "ldrh", "ldrsh"][((h >> 10) & 3) as usize]
            };
            format!("{op} {}, [{}, {}]", low(0), low(3), low(6))
        }
        _ => unreachable!("formats 1 to 8 only"),
    }
}

fn thumb_offset(op: &str, rd: &str, rb: &str, offset: u32) -> String {
    if offset == 0 {
        format!("{op} {rd}, [{rb}]")
    } else {
        format!("{op} {rd}, [{rb}, {}]", imm(offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(word: u32) -> String {
        arm(0x0800_0000, word)
    }

    fn t(half: u16) -> String {
        thumb(0x0800_0000, half, 0).0
    }

    #[test]
    fn arm_data_processing() {
        assert_eq!(a(0xE3A0_0301), "mov r0, #0x4000000");
        assert_eq!(a(0xE092_1003), "adds r1, r2, r3");
        assert_eq!(a(0x0042_1083), "subeq r1, r2, r3, lsl #1");
        assert_eq!(a(0xE1A0_0231), "mov r0, r1, lsr r2");
        assert_eq!(a(0xE1A0_0061), "mov r0, r1, rrx");
        assert_eq!(a(0xE1A0_0021), "mov r0, r1, lsr #32");
        assert_eq!(a(0xE355_0000), "cmp r5, #0");
        assert_eq!(a(0x11E0_0000), "mvnne r0, r0");
    }

    #[test]
    fn arm_branches_and_system() {
        assert_eq!(a(0xEA00_002E), "b 0x080000c0");
        assert_eq!(a(0xEBFF_FFFE), "bl 0x08000000");
        assert_eq!(a(0xE12F_FF1E), "bx lr");
        assert_eq!(a(0xEF05_0000), "swi #0x50000");
        assert_eq!(a(0xE10F_0000), "mrs r0, cpsr");
        assert_eq!(a(0xE129_F000), "msr cpsr_fc, r0");
        assert_eq!(a(0xE321_F01F), "msr cpsr_c, #0x1f");
        assert_eq!(a(0xEE00_0000), "undefined", "no coprocessors");
        assert_eq!(a(0xF1A0_0000), "movnv r0, r0", "never runs, still decodes");
    }

    #[test]
    fn arm_multiply_and_swap() {
        assert_eq!(a(0xE000_0291), "mul r0, r1, r2");
        assert_eq!(a(0xE021_3492), "mla r1, r2, r4, r3");
        assert_eq!(a(0xE0C1_0392), "smull r0, r1, r2, r3");
        assert_eq!(a(0xE0A1_0392), "umlal r0, r1, r2, r3");
        assert_eq!(a(0xE142_1093), "swpb r1, r3, [r2]");
    }

    #[test]
    fn arm_loads_and_stores() {
        assert_eq!(a(0xE59F_0010), "ldr r0, [pc, #0x10] ; 0x08000018");
        assert_eq!(a(0xE5D1_2000), "ldrb r2, [r1]");
        assert_eq!(a(0xE5A1_2004), "str r2, [r1, #4]!");
        assert_eq!(a(0xE491_2004), "ldr r2, [r1], #4");
        assert_eq!(a(0xE511_2004), "ldr r2, [r1, #-4]");
        assert_eq!(a(0xE791_2103), "ldr r2, [r1, r3, lsl #2]");
        assert_eq!(a(0xE1D1_20B2), "ldrh r2, [r1, #2]");
        assert_eq!(a(0xE191_20D3), "ldrsb r2, [r1, r3]");
        assert_eq!(a(0xE0D1_20F2), "ldrsh r2, [r1], #2");
        assert_eq!(a(0xE1C1_20F0), "undefined", "strd is ARMv5");
    }

    #[test]
    fn arm_block_transfers() {
        assert_eq!(a(0xE92D_4010), "push {r4, lr}");
        assert_eq!(a(0xE8BD_800F), "pop {r0-r3, pc}");
        assert_eq!(a(0xE891_000C), "ldm r1, {r2, r3}");
        assert_eq!(a(0xE9C0_8000), "stmib r0, {pc}^");
    }

    #[test]
    fn thumb_alu_and_moves() {
        assert_eq!(t(0x2001), "movs r0, #1");
        assert_eq!(t(0x0088), "lsls r0, r1, #2");
        assert_eq!(t(0x0008), "movs r0, r1");
        assert_eq!(t(0x0808), "lsrs r0, r1, #32");
        assert_eq!(t(0x1888), "adds r0, r1, r2");
        assert_eq!(t(0x1E48), "subs r0, r1, #1");
        assert_eq!(t(0x4348), "muls r0, r1");
        assert_eq!(t(0x46C0), "mov r8, r8");
        assert_eq!(t(0x4770), "bx lr");
        assert_eq!(t(0x2CFF), "cmp r4, #0xff");
    }

    #[test]
    fn thumb_memory() {
        // PC is word-aligned: 0x08000000 + 4 = 0x08000004.
        assert_eq!(t(0x4801), "ldr r0, [pc, #4] ; 0x08000008");
        assert_eq!(t(0x5888), "ldr r0, [r1, r2]");
        assert_eq!(t(0x5E88), "ldrsh r0, [r1, r2]");
        assert_eq!(t(0x6848), "ldr r0, [r1, #4]");
        assert_eq!(t(0x7808), "ldrb r0, [r1]");
        assert_eq!(t(0x8848), "ldrh r0, [r1, #2]");
        assert_eq!(t(0x9001), "str r0, [sp, #4]");
        assert_eq!(t(0xB5F0), "push {r4-r7, lr}");
        assert_eq!(t(0xBD01), "pop {r0, pc}");
        assert_eq!(t(0xC10C), "stmia r1!, {r2, r3}");
        assert_eq!(t(0xC90C), "ldmia r1!, {r2, r3}");
        assert_eq!(t(0xC906), "ldmia r1, {r1, r2}", "base loaded: no writeback");
        assert_eq!(t(0xB082), "sub sp, #8");
        assert_eq!(t(0xA001), "add r0, pc, #4 ; 0x08000008");
    }

    #[test]
    fn thumb_branches() {
        assert_eq!(t(0xD0FE), "beq 0x08000000");
        assert_eq!(t(0xE7FE), "b 0x08000000");
        assert_eq!(t(0xDF05), "swi #5");
        // bl to 0x08001000: offset 0xFFC = (0 << 12) + (0x7FE << 1).
        assert_eq!(
            thumb(0x0800_0000, 0xF000, 0xFFFE),
            ("bl 0x08001000".into(), 4)
        );
        // Backwards: bl to 0x08000000 from 0x08000100.
        assert_eq!(thumb(0x0800_0100, 0xF7FF, 0xFF7E).0, "bl 0x08000000");
        assert_eq!(t(0xF800), "bl.lo lr, #0");
        assert_eq!(t(0xDE00), "undefined");
    }

    #[test]
    fn register_lists() {
        assert_eq!(reg_list(0b1), "{r0}");
        assert_eq!(reg_list(0b11), "{r0, r1}");
        assert_eq!(reg_list(0b1011_1000_0000_0111), "{r0-r2, r11, r12, sp, pc}");
    }
}
