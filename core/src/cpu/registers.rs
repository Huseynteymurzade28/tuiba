//! ARM7TDMI register file: general-purpose registers with per-mode banking,
//! the CPSR and the banked SPSRs.

/// Program counter register index.
pub const PC: usize = 15;
/// Link register index.
pub const LR: usize = 14;
/// Stack pointer register index.
pub const SP: usize = 13;

/// Processor operating mode (CPSR bits 4:0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Mode {
    /// Unprivileged mode games normally run in.
    User = 0x10,
    /// Fast interrupt; has its own r8–r14.
    Fiq = 0x11,
    /// Normal interrupt.
    Irq = 0x12,
    /// Entered by `SWI` and at reset.
    Supervisor = 0x13,
    /// Memory abort (not raised by GBA hardware).
    Abort = 0x17,
    /// Undefined instruction trap.
    Undefined = 0x1B,
    /// Privileged mode sharing User's registers.
    System = 0x1F,
}

impl Mode {
    /// Decodes the mode bits, returning `None` for reserved encodings.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        Some(match bits & 0x1F {
            0x10 => Self::User,
            0x11 => Self::Fiq,
            0x12 => Self::Irq,
            0x13 => Self::Supervisor,
            0x17 => Self::Abort,
            0x1B => Self::Undefined,
            0x1F => Self::System,
            _ => return None,
        })
    }

    /// Index into the banked-register arrays.
    ///
    /// User and System share a bank; the privileged exception modes each
    /// get their own.
    #[must_use]
    pub const fn bank(self) -> usize {
        match self {
            Self::User | Self::System => 0,
            Self::Fiq => 1,
            Self::Irq => 2,
            Self::Supervisor => 3,
            Self::Abort => 4,
            Self::Undefined => 5,
        }
    }

    /// Whether this mode has an SPSR (all but User/System).
    #[must_use]
    pub const fn has_spsr(self) -> bool {
        !matches!(self, Self::User | Self::System)
    }
}

/// Number of register banks (see [`Mode::bank`]).
const BANKS: usize = 6;

/// The Current Program Status Register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Cpsr(pub u32);

impl Cpsr {
    const N: u32 = 1 << 31;
    const Z: u32 = 1 << 30;
    const C: u32 = 1 << 29;
    const V: u32 = 1 << 28;
    const I: u32 = 1 << 7;
    const F: u32 = 1 << 6;
    const T: u32 = 1 << 5;
    const MODE_MASK: u32 = 0x1F;

    /// Negative flag.
    #[must_use]
    pub const fn n(self) -> bool {
        self.0 & Self::N != 0
    }
    /// Zero flag.
    #[must_use]
    pub const fn z(self) -> bool {
        self.0 & Self::Z != 0
    }
    /// Carry flag.
    #[must_use]
    pub const fn c(self) -> bool {
        self.0 & Self::C != 0
    }
    /// Overflow flag.
    #[must_use]
    pub const fn v(self) -> bool {
        self.0 & Self::V != 0
    }
    /// IRQ disable.
    #[must_use]
    pub const fn irq_disabled(self) -> bool {
        self.0 & Self::I != 0
    }
    /// FIQ disable.
    #[must_use]
    pub const fn fiq_disabled(self) -> bool {
        self.0 & Self::F != 0
    }
    /// THUMB state.
    #[must_use]
    pub const fn thumb(self) -> bool {
        self.0 & Self::T != 0
    }
    /// Current mode, or `None` if the mode bits hold a reserved value.
    #[must_use]
    pub const fn mode(self) -> Option<Mode> {
        Mode::from_bits(self.0 & Self::MODE_MASK)
    }

    fn set_bit(&mut self, bit: u32, on: bool) {
        if on {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    /// Sets the negative flag.
    pub fn set_n(&mut self, on: bool) {
        self.set_bit(Self::N, on);
    }
    /// Sets the zero flag.
    pub fn set_z(&mut self, on: bool) {
        self.set_bit(Self::Z, on);
    }
    /// Sets the carry flag.
    pub fn set_c(&mut self, on: bool) {
        self.set_bit(Self::C, on);
    }
    /// Sets the overflow flag.
    pub fn set_v(&mut self, on: bool) {
        self.set_bit(Self::V, on);
    }
    /// Sets N and Z from a result, the common case for logical ops.
    pub fn set_nz(&mut self, result: u32) {
        self.set_n(result & Self::N != 0);
        self.set_z(result == 0);
    }
    /// Sets the IRQ disable bit.
    pub fn set_irq_disabled(&mut self, on: bool) {
        self.set_bit(Self::I, on);
    }
    /// Sets the FIQ disable bit.
    pub fn set_fiq_disabled(&mut self, on: bool) {
        self.set_bit(Self::F, on);
    }
    /// Sets the THUMB bit. Callers must also flush the pipeline.
    pub fn set_thumb(&mut self, on: bool) {
        self.set_bit(Self::T, on);
    }
    /// Sets the mode bits. Callers must bank registers via
    /// [`Registers::switch_mode`] rather than calling this directly.
    pub(crate) fn set_mode_bits(&mut self, mode: Mode) {
        self.0 = (self.0 & !Self::MODE_MASK) | mode as u32;
    }

    /// Evaluates an ARM condition code (bits 31:28 of an ARM opcode, or the
    /// THUMB conditional-branch field) against the flags.
    #[must_use]
    pub const fn condition(self, cond: u32) -> bool {
        let (n, z, c, v) = (self.n(), self.z(), self.c(), self.v());
        match cond & 0xF {
            0x0 => z,            // EQ
            0x1 => !z,           // NE
            0x2 => c,            // CS/HS
            0x3 => !c,           // CC/LO
            0x4 => n,            // MI
            0x5 => !n,           // PL
            0x6 => v,            // VS
            0x7 => !v,           // VC
            0x8 => c && !z,      // HI
            0x9 => !c || z,      // LS
            0xA => n == v,       // GE
            0xB => n != v,       // LT
            0xC => !z && n == v, // GT
            0xD => z || n != v,  // LE
            0xE => true,         // AL
            // NV: never on ARMv3, but ARM7TDMI treats it as unconditional
            // for the few v5 encodings that use it. We follow "never".
            _ => false,
        }
    }
}

/// The complete visible register state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Registers {
    /// r0–r15 as seen in the current mode.
    gpr: [u32; 16],
    /// The CPSR.
    pub cpsr: Cpsr,
    /// r8–r12 for every mode except FIQ, saved while FIQ is active.
    usr_r8_r12: [u32; 5],
    /// FIQ's private r8–r12, saved while any other mode is active.
    fiq_r8_r12: [u32; 5],
    /// r13 (SP) per bank.
    banked_sp: [u32; BANKS],
    /// r14 (LR) per bank.
    banked_lr: [u32; BANKS],
    /// SPSR per bank. Bank 0 (User/System) is unused.
    banked_spsr: [Cpsr; BANKS],
}

impl Default for Registers {
    fn default() -> Self {
        Self::new()
    }
}

impl Registers {
    /// Registers in the reset state: Supervisor mode, ARM, interrupts off.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gpr: [0; 16],
            cpsr: Cpsr(Cpsr::I | Cpsr::F | Mode::Supervisor as u32),
            usr_r8_r12: [0; 5],
            fiq_r8_r12: [0; 5],
            banked_sp: [0; BANKS],
            banked_lr: [0; BANKS],
            banked_spsr: [Cpsr(0); BANKS],
        }
    }

    /// Reads register `index` (0–15) in the current mode.
    #[inline]
    #[must_use]
    pub const fn get(&self, index: usize) -> u32 {
        self.gpr[index]
    }

    /// Writes register `index` (0–15) in the current mode.
    ///
    /// Writing r15 here does **not** flush the pipeline; the CPU core
    /// handles that.
    #[inline]
    pub const fn set(&mut self, index: usize, value: u32) {
        self.gpr[index] = value;
    }

    /// Current mode.
    ///
    /// # Panics
    ///
    /// Panics if the mode bits hold a reserved value. Every writer of the
    /// CPSR goes through [`Registers::set_cpsr`], which rejects such
    /// values, so this cannot happen in practice.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.cpsr.mode().expect("CPSR mode bits are always valid")
    }

    /// The SPSR of the current mode, or `None` in User/System.
    #[must_use]
    pub fn spsr(&self) -> Option<Cpsr> {
        let mode = self.mode();
        mode.has_spsr().then(|| self.banked_spsr[mode.bank()])
    }

    /// Writes the SPSR of the current mode. No-op in User/System.
    pub fn set_spsr(&mut self, value: Cpsr) {
        let mode = self.mode();
        if mode.has_spsr() {
            self.banked_spsr[mode.bank()] = value;
        }
    }

    /// Reads a User-mode register regardless of the current mode
    /// (for `LDM`/`STM` with the `^` suffix).
    #[must_use]
    pub fn get_user(&self, index: usize) -> u32 {
        let mode = self.mode();
        match index {
            8..=12 if mode == Mode::Fiq => self.usr_r8_r12[index - 8],
            SP if mode.bank() != 0 => self.banked_sp[0],
            LR if mode.bank() != 0 => self.banked_lr[0],
            _ => self.gpr[index],
        }
    }

    /// Writes a User-mode register regardless of the current mode.
    pub fn set_user(&mut self, index: usize, value: u32) {
        let mode = self.mode();
        match index {
            8..=12 if mode == Mode::Fiq => self.usr_r8_r12[index - 8] = value,
            SP if mode.bank() != 0 => self.banked_sp[0] = value,
            LR if mode.bank() != 0 => self.banked_lr[0] = value,
            _ => self.gpr[index] = value,
        }
    }

    /// Switches to `new`, banking r13/r14 (and r8–r12 for FIQ) so that the
    /// visible `gpr` array reflects the new mode.
    pub fn switch_mode(&mut self, new: Mode) {
        let old = self.mode();
        if old == new {
            return;
        }

        let (ob, nb) = (old.bank(), new.bank());
        if ob != nb {
            self.banked_sp[ob] = self.gpr[SP];
            self.banked_lr[ob] = self.gpr[LR];
            self.gpr[SP] = self.banked_sp[nb];
            self.gpr[LR] = self.banked_lr[nb];
        }

        match (old == Mode::Fiq, new == Mode::Fiq) {
            (false, true) => {
                self.usr_r8_r12.copy_from_slice(&self.gpr[8..13]);
                self.gpr[8..13].copy_from_slice(&self.fiq_r8_r12);
            }
            (true, false) => {
                self.fiq_r8_r12.copy_from_slice(&self.gpr[8..13]);
                self.gpr[8..13].copy_from_slice(&self.usr_r8_r12);
            }
            _ => {}
        }

        self.cpsr.set_mode_bits(new);
    }

    /// Replaces the whole CPSR, switching modes if the mode bits changed.
    ///
    /// Reserved mode encodings leave the mode unchanged, matching the
    /// "unpredictable" hardware behaviour in the least surprising way.
    pub fn set_cpsr(&mut self, value: Cpsr) {
        let target = value.mode().unwrap_or_else(|| self.mode());
        self.switch_mode(target);
        self.cpsr.0 = (value.0 & !Cpsr::MODE_MASK) | (self.cpsr.0 & Cpsr::MODE_MASK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state() {
        let r = Registers::new();
        assert_eq!(r.mode(), Mode::Supervisor);
        assert!(r.cpsr.irq_disabled());
        assert!(r.cpsr.fiq_disabled());
        assert!(!r.cpsr.thumb());
    }

    #[test]
    fn condition_codes() {
        let mut c = Cpsr(0);
        assert!(c.condition(0xE));
        assert!(!c.condition(0xF));
        assert!(c.condition(0x1)); // NE
        c.set_z(true);
        assert!(c.condition(0x0)); // EQ
        assert!(c.condition(0x9)); // LS
        assert!(!c.condition(0x8)); // HI
        assert!(c.condition(0xD)); // LE
        c.set_z(false);
        c.set_n(true);
        c.set_v(false);
        assert!(c.condition(0xB)); // LT
        assert!(!c.condition(0xA)); // GE
        c.set_v(true);
        assert!(c.condition(0xC)); // GT
    }

    #[test]
    fn set_nz_from_result() {
        let mut c = Cpsr(0);
        c.set_nz(0);
        assert!(c.z() && !c.n());
        c.set_nz(0x8000_0000);
        assert!(!c.z() && c.n());
    }

    #[test]
    fn sp_lr_are_banked_per_mode() {
        let mut r = Registers::new();
        r.set(SP, 0x1000);
        r.set(LR, 0x1004);
        r.switch_mode(Mode::Irq);
        assert_eq!(r.get(SP), 0);
        r.set(SP, 0x2000);
        r.switch_mode(Mode::Supervisor);
        assert_eq!(r.get(SP), 0x1000);
        assert_eq!(r.get(LR), 0x1004);
        r.switch_mode(Mode::Irq);
        assert_eq!(r.get(SP), 0x2000);
        assert_eq!(r.mode(), Mode::Irq);
    }

    #[test]
    fn user_and_system_share_a_bank() {
        let mut r = Registers::new();
        r.switch_mode(Mode::System);
        r.set(SP, 0x3000);
        r.switch_mode(Mode::User);
        assert_eq!(r.get(SP), 0x3000);
    }

    #[test]
    fn fiq_banks_r8_to_r12() {
        let mut r = Registers::new();
        for i in 8..13 {
            r.set(i, i as u32);
        }
        r.switch_mode(Mode::Fiq);
        for i in 8..13 {
            assert_eq!(r.get(i), 0);
            r.set(i, 0xF0 + i as u32);
        }
        assert_eq!(r.get_user(10), 10);
        r.switch_mode(Mode::User);
        for i in 8..13 {
            assert_eq!(r.get(i), i as u32);
        }
        r.switch_mode(Mode::Fiq);
        assert_eq!(r.get(10), 0xFA);
    }

    #[test]
    fn user_accessors_reach_through_bank() {
        let mut r = Registers::new();
        r.switch_mode(Mode::System);
        r.set(SP, 0xAAAA);
        r.switch_mode(Mode::Irq);
        r.set(SP, 0xBBBB);
        assert_eq!(r.get_user(SP), 0xAAAA);
        r.set_user(SP, 0xCCCC);
        assert_eq!(r.get(SP), 0xBBBB);
        r.switch_mode(Mode::User);
        assert_eq!(r.get(SP), 0xCCCC);
    }

    #[test]
    fn spsr_only_in_exception_modes() {
        let mut r = Registers::new();
        r.set_spsr(Cpsr(0x1234_0010));
        assert_eq!(r.spsr(), Some(Cpsr(0x1234_0010)));
        r.switch_mode(Mode::User);
        assert_eq!(r.spsr(), None);
        r.set_spsr(Cpsr(1));
        r.switch_mode(Mode::Supervisor);
        assert_eq!(r.spsr(), Some(Cpsr(0x1234_0010)));
    }

    #[test]
    fn set_cpsr_switches_mode_and_keeps_flags() {
        let mut r = Registers::new();
        r.set(SP, 0x10);
        r.set_cpsr(Cpsr(0x8000_0012));
        assert_eq!(r.mode(), Mode::Irq);
        assert!(r.cpsr.n());
        assert_eq!(r.get(SP), 0);
        // Reserved mode bits: flags applied, mode retained.
        r.set_cpsr(Cpsr(0x4000_0000));
        assert_eq!(r.mode(), Mode::Irq);
        assert!(r.cpsr.z() && !r.cpsr.n());
    }
}
