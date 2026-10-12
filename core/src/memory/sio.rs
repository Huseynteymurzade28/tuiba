//! The serial (link) port, with no cable plugged in.
//!
//! The port's mode is picked by `RCNT` bits 15–14 and then `SIOCNT`
//! bits 13–12:
//!
//! | `RCNT` 15–14 | `SIOCNT` 13–12 | Mode                     |
//! | ------------ | -------------- | ------------------------ |
//! | `0x`         | `00`           | normal, 8-bit            |
//! | `0x`         | `01`           | normal, 32-bit           |
//! | `0x`         | `10`           | multiplayer              |
//! | `0x`         | `11`           | UART                     |
//! | `10`         | —              | general-purpose pins     |
//! | `11`         | —              | JOY bus                  |
//!
//! Nothing answers on the other end, which games tell apart from a
//! partner by what the port reports rather than by a timeout:
//!
//! - Normal mode, internal clock: the GBA is the master and clocks the
//!   transfer itself, so it finishes — the start bit drops, the serial
//!   interrupt fires if enabled, and the pulled-up input line shifts in
//!   all ones. An external-clock transfer waits for a master that never
//!   comes and stays busy, as on hardware.
//! - Multiplayer: the SI and SD terminals both read high (a child with
//!   every unit "ready"), ID and error bits read zero, and the transfer
//!   is never started — only the parent may start one.
//!
//! The registers keep their plain storage in [`super::IoRegisters`];
//! this only rewrites `SIOCNT` writes.

/// Serial port mode, from `RCNT` and `SIOCNT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal mode, 8-bit transfers.
    Normal8,
    /// Normal mode, 32-bit transfers.
    Normal32,
    /// Multiplayer (up to four units).
    Multiplayer,
    /// UART.
    Uart,
    /// General-purpose pins.
    GeneralPurpose,
    /// JOY bus.
    JoyBus,
}

impl Mode {
    /// The mode selected by the two control registers.
    #[must_use]
    pub const fn from_registers(rcnt: u16, siocnt: u16) -> Self {
        if rcnt & 0x8000 != 0 {
            return if rcnt & 0x4000 != 0 {
                Self::JoyBus
            } else {
                Self::GeneralPurpose
            };
        }
        match (siocnt >> 12) & 3 {
            0 => Self::Normal8,
            1 => Self::Normal32,
            2 => Self::Multiplayer,
            _ => Self::Uart,
        }
    }
}

/// `SIOCNT` bit 0 in normal mode: the GBA supplies the clock.
const INTERNAL_CLOCK: u16 = 1 << 0;
/// `SIOCNT` bit 2: the SI terminal, read-only. High with nothing attached.
const SI: u16 = 1 << 2;
/// `SIOCNT` bit 3 in multiplayer mode: the SD terminal, read-only.
const SD: u16 = 1 << 3;
/// `SIOCNT` bit 7: start / busy.
const START: u16 = 1 << 7;
/// `SIOCNT` bit 14: raise the serial interrupt when a transfer ends.
pub const IRQ_ENABLE: u16 = 1 << 14;

/// What a write to `SIOCNT` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// The value the register holds afterwards.
    pub siocnt: u16,
    /// A normal-mode transfer finished: the data registers now hold what
    /// was shifted in, and the interrupt is due if [`IRQ_ENABLE`] is set.
    pub finished: Option<Mode>,
}

/// Applies a software write of `value` to `SIOCNT`.
#[must_use]
pub const fn write_siocnt(value: u16, rcnt: u16) -> Written {
    let mode = Mode::from_registers(rcnt, value);
    match mode {
        Mode::Normal8 | Mode::Normal32 => {
            let value = value | SI;
            if value & (START | INTERNAL_CLOCK) == START | INTERNAL_CLOCK {
                Written {
                    siocnt: value & !START,
                    finished: Some(mode),
                }
            } else {
                Written {
                    siocnt: value,
                    finished: None,
                }
            }
        }
        // Keep baud rate, start and the writable high bits; bits 2–6
        // are the terminals, the unit ID and the error flag.
        Mode::Multiplayer => Written {
            siocnt: (value & 0xFF83) | SI | SD,
            finished: None,
        },
        Mode::Uart | Mode::GeneralPurpose | Mode::JoyBus => Written {
            siocnt: value,
            finished: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_follows_rcnt_then_siocnt() {
        assert_eq!(Mode::from_registers(0, 0x0000), Mode::Normal8);
        assert_eq!(Mode::from_registers(0, 0x1000), Mode::Normal32);
        assert_eq!(Mode::from_registers(0, 0x2000), Mode::Multiplayer);
        assert_eq!(Mode::from_registers(0, 0x3000), Mode::Uart);
        assert_eq!(Mode::from_registers(0x8000, 0x2000), Mode::GeneralPurpose);
        assert_eq!(Mode::from_registers(0xC000, 0x2000), Mode::JoyBus);
    }

    #[test]
    fn internal_clock_transfer_finishes_at_once() {
        let w = write_siocnt(0x4081, 0);
        assert_eq!(w.siocnt, 0x4005, "start cleared, SI high");
        assert_eq!(w.finished, Some(Mode::Normal8));
        let w = write_siocnt(0x1081, 0);
        assert_eq!(w.finished, Some(Mode::Normal32));
    }

    #[test]
    fn external_clock_transfer_stays_busy() {
        let w = write_siocnt(0x0080, 0);
        assert_eq!(w.siocnt, 0x0084);
        assert_eq!(w.finished, None);
    }

    #[test]
    fn multiplayer_reports_a_lone_child() {
        let w = write_siocnt(0x60F3, 0);
        assert_eq!(w.siocnt, 0x608F, "SI+SD high, ID and error clear");
        assert_eq!(w.finished, None);
    }
}
