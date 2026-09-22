//! Bus access timing: how many cycles each region takes to answer.
//!
//! Every access costs at least one cycle. Slow regions add *wait states*:
//! EWRAM and the cartridge sit on a 16-bit bus, so a 32-bit access is two
//! halfword accesses back to back, and the cartridge distinguishes
//! *non-sequential* (`N`) accesses from *sequential* (`S`) ones that
//! continue where the previous access ended. `WAITCNT` lets software tune
//! the cartridge and SRAM timings; everything else is fixed.

/// Cycle cost of accessing one region, by width and sequentiality.
/// Byte accesses cost the same as halfword accesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Access {
    /// Non-sequential 8/16-bit.
    pub n16: u8,
    /// Sequential 8/16-bit.
    pub s16: u8,
    /// Non-sequential 32-bit.
    pub n32: u8,
    /// Sequential 32-bit.
    pub s32: u8,
}

impl Access {
    /// A region whose cost never depends on history.
    const fn fixed(c16: u8, c32: u8) -> Self {
        Self {
            n16: c16,
            s16: c16,
            n32: c32,
            s32: c32,
        }
    }

    /// A 16-bit-bus region: a word is one `N` then one `S` halfword.
    const fn halfword_bus(n: u8, s: u8) -> Self {
        Self {
            n16: n,
            s16: s,
            n32: n + s,
            s32: s + s,
        }
    }

    /// Cost of an access of `width` bytes.
    #[inline]
    #[must_use]
    pub const fn cycles(self, width: u32, sequential: bool) -> u32 {
        let c = match (width, sequential) {
            (4, false) => self.n32,
            (4, true) => self.s32,
            (_, false) => self.n16,
            (_, true) => self.s16,
        };
        c as u32
    }
}

/// Access costs for every 16 MiB page of the address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaitStates {
    pages: [Access; 16],
    /// `WAITCNT` bit 14: the cartridge prefetch buffer is enabled.
    pub prefetch: bool,
}

impl Default for WaitStates {
    fn default() -> Self {
        Self::from_waitcnt(0)
    }
}

impl WaitStates {
    /// Decodes `WAITCNT`.
    #[must_use]
    pub fn from_waitcnt(value: u16) -> Self {
        // Wait states (not counting the access cycle itself).
        const FIRST: [u8; 4] = [4, 3, 2, 8];
        let first = |shift: u32| FIRST[usize::from((value >> shift) & 0b11)] + 1;
        let second = |shift: u32, slow: u8| {
            if value & (1 << shift) != 0 {
                2
            } else {
                slow + 1
            }
        };

        let sram = Access::fixed(first(0), first(0));
        let ws0 = Access::halfword_bus(first(2), second(4, 2));
        let ws1 = Access::halfword_bus(first(5), second(7, 4));
        let ws2 = Access::halfword_bus(first(8), second(10, 8));

        let one = Access::fixed(1, 1);
        let video = Access::fixed(1, 2);
        let pages = [
            one,                        // 0x00 BIOS
            one,                        // 0x01 unused
            Access::halfword_bus(3, 3), // 0x02 EWRAM
            one,                        // 0x03 IWRAM
            one,                        // 0x04 I/O
            video,                      // 0x05 palette
            video,                      // 0x06 VRAM
            one,                        // 0x07 OAM
            ws0,
            ws0, // 0x08–0x09
            ws1,
            ws1, // 0x0A–0x0B
            ws2,
            ws2, // 0x0C–0x0D
            sram,
            sram, // 0x0E–0x0F
        ];
        Self {
            pages,
            prefetch: value & (1 << 14) != 0,
        }
    }

    /// The access costs of the page containing `address`.
    #[inline]
    #[must_use]
    pub fn page(&self, address: u32) -> Access {
        self.pages[(address >> 24) as usize & 0xF]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_defaults() {
        let w = WaitStates::default();
        assert!(!w.prefetch);
        assert_eq!(w.page(0x0000_0000), Access::fixed(1, 1), "BIOS");
        assert_eq!(
            w.page(0x0200_0000),
            Access {
                n16: 3,
                s16: 3,
                n32: 6,
                s32: 6
            },
            "EWRAM"
        );
        assert_eq!(w.page(0x0300_0000), Access::fixed(1, 1), "IWRAM");
        assert_eq!(w.page(0x0500_0000), Access::fixed(1, 2), "palette");
        assert_eq!(w.page(0x0600_0000), Access::fixed(1, 2), "VRAM");
        assert_eq!(w.page(0x0700_0000), Access::fixed(1, 1), "OAM");
        assert_eq!(
            w.page(0x0800_0000),
            Access {
                n16: 5,
                s16: 3,
                n32: 8,
                s32: 6
            },
            "WS0"
        );
        assert_eq!(
            w.page(0x0A00_0000),
            Access {
                n16: 5,
                s16: 5,
                n32: 10,
                s32: 10
            },
            "WS1"
        );
        assert_eq!(
            w.page(0x0C00_0000),
            Access {
                n16: 5,
                s16: 9,
                n32: 14,
                s32: 18
            },
            "WS2"
        );
        assert_eq!(w.page(0x0E00_0000), Access::fixed(5, 5), "SRAM");
        assert_eq!(w.page(0x0F00_0000), Access::fixed(5, 5), "SRAM mirror");
    }

    #[test]
    fn decodes_the_usual_game_setting() {
        // SRAM 8 waits, WS0 3/1, WS1 4/4, WS2 8/8, prefetch on.
        let w = WaitStates::from_waitcnt(0x4317);
        assert!(w.prefetch);
        assert_eq!(w.page(0x0E00_0000).n16, 9);
        assert_eq!(
            w.page(0x0800_0000),
            Access {
                n16: 4,
                s16: 2,
                n32: 6,
                s32: 4
            }
        );
        assert_eq!(
            w.page(0x0900_0000),
            w.page(0x0800_0000),
            "upper half of WS0"
        );
        assert_eq!(
            w.page(0x0A00_0000),
            Access {
                n16: 5,
                s16: 5,
                n32: 10,
                s32: 10
            }
        );
        assert_eq!(
            w.page(0x0C00_0000),
            Access {
                n16: 9,
                s16: 9,
                n32: 18,
                s32: 18
            }
        );
    }

    #[test]
    fn cycles_by_width_and_sequentiality() {
        let a = Access {
            n16: 5,
            s16: 3,
            n32: 8,
            s32: 6,
        };
        assert_eq!(a.cycles(1, false), 5);
        assert_eq!(a.cycles(2, true), 3);
        assert_eq!(a.cycles(4, false), 8);
        assert_eq!(a.cycles(4, true), 6);
    }
}
