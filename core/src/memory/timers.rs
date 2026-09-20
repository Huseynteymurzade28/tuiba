//! The four 16-bit timers.
//!
//! Each timer counts up at the system clock divided by a prescaler, or
//! (for timers 1–3) once per overflow of the previous timer. On overflow
//! it reloads from `TMxCNT_L` and may raise an interrupt.

use crate::memory::io::Interrupt;

/// State of one timer.
#[derive(Debug, Clone, Copy, Default)]
struct Timer {
    /// Value written to `TMxCNT_L`, loaded on enable and on overflow.
    reload: u16,
    /// Current count, readable through `TMxCNT_L`.
    counter: u16,
    /// Raw `TMxCNT_H`.
    control: u16,
    /// Accumulated cycles not yet converted into ticks.
    fraction: u32,
}

impl Timer {
    const ENABLE: u16 = 1 << 7;
    const IRQ: u16 = 1 << 6;
    const CASCADE: u16 = 1 << 2;

    fn enabled(self) -> bool {
        self.control & Self::ENABLE != 0
    }

    fn cascade(self) -> bool {
        self.control & Self::CASCADE != 0
    }

    fn prescaler(self) -> u32 {
        match self.control & 0b11 {
            0 => 1,
            1 => 64,
            2 => 256,
            _ => 1024,
        }
    }

    /// Advances by `ticks`; returns how many times the timer overflowed.
    fn tick(&mut self, ticks: u32) -> u32 {
        let mut overflows = 0;
        let mut remaining = ticks;
        while remaining > 0 {
            let to_overflow = u32::from(u16::MAX - self.counter) + 1;
            if remaining < to_overflow {
                self.counter += remaining as u16;
                break;
            }
            remaining -= to_overflow;
            self.counter = self.reload;
            overflows += 1;
        }
        overflows
    }
}

/// All four timers.
#[derive(Debug, Clone, Default)]
pub struct Timers {
    timers: [Timer; 4],
}

impl Timers {
    /// Timers in their power-on state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Current counter of timer `n` (`TMxCNT_L` read).
    #[must_use]
    pub fn counter(&self, n: usize) -> u16 {
        self.timers[n].counter
    }

    /// Control register of timer `n` (`TMxCNT_H` read).
    #[must_use]
    pub fn control(&self, n: usize) -> u16 {
        self.timers[n].control
    }

    /// Sets the reload value (`TMxCNT_L` write). Takes effect on the next
    /// enable or overflow.
    pub fn set_reload(&mut self, n: usize, value: u16) {
        self.timers[n].reload = value;
    }

    /// Writes `TMxCNT_H`. Enabling a stopped timer loads the counter.
    pub fn set_control(&mut self, n: usize, value: u16) {
        let timer = &mut self.timers[n];
        let was_enabled = timer.enabled();
        timer.control = value & 0x00C7;
        if !was_enabled && timer.enabled() {
            timer.counter = timer.reload;
            timer.fraction = 0;
        }
    }

    /// Advances all timers by `cycles`. Returns a bit mask (bit `n` = timer
    /// `n`) of timers that overflowed with their IRQ enabled; see
    /// [`Timers::interrupt`].
    pub fn step(&mut self, cycles: u32) -> u8 {
        let mut irqs = 0u8;
        let mut carried = 0;
        for n in 0..4 {
            let timer = &mut self.timers[n];
            let overflows = if !timer.enabled() {
                0
            } else if timer.cascade() && n > 0 {
                timer.tick(carried)
            } else {
                timer.fraction += cycles;
                let ticks = timer.fraction / timer.prescaler();
                timer.fraction %= timer.prescaler();
                timer.tick(ticks)
            };
            if overflows > 0 && timer.control & Timer::IRQ != 0 {
                irqs |= 1 << n;
            }
            carried = overflows;
        }
        irqs
    }

    /// The interrupt source for timer `n`.
    #[must_use]
    pub const fn interrupt(n: usize) -> Interrupt {
        match n {
            0 => Interrupt::Timer0,
            1 => Interrupt::Timer1,
            2 => Interrupt::Timer2,
            _ => Interrupt::Timer3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_with_prescaler_and_reloads_on_overflow() {
        let mut t = Timers::new();
        t.set_reload(0, 0xFFF0);
        t.set_control(0, 0x80 | 1); // enable, /64
        assert_eq!(t.counter(0), 0xFFF0);
        assert_eq!(t.step(63), 0);
        assert_eq!(t.counter(0), 0xFFF0);
        t.step(1);
        assert_eq!(t.counter(0), 0xFFF1);
        // 15 more ticks overflow: 0xFFF1 + 15 -> wraps to reload
        assert_eq!(t.step(15 * 64), 0, "IRQ bit not set");
        assert_eq!(t.counter(0), 0xFFF0);
    }

    #[test]
    fn overflow_raises_irq_and_cascades() {
        let mut t = Timers::new();
        t.set_reload(0, 0xFFFF);
        t.set_control(0, 0x80 | 0x40); // enable, IRQ, /1
        t.set_reload(1, 0xFFFE);
        t.set_control(1, 0x80 | 0x40 | 0x04); // enable, IRQ, cascade
        assert_eq!(t.step(1), 0b01);
        assert_eq!(t.counter(1), 0xFFFF, "cascaded once");
        assert_eq!(t.step(1), 0b11);
        assert_eq!(Timers::interrupt(1), Interrupt::Timer1);
        assert_eq!(t.counter(1), 0xFFFE);
    }

    #[test]
    fn disabled_timer_does_not_count() {
        let mut t = Timers::new();
        t.set_reload(2, 10);
        t.step(1000);
        assert_eq!(t.counter(2), 0);
        t.set_control(2, 0x80);
        assert_eq!(t.counter(2), 10);
        t.step(5);
        assert_eq!(t.counter(2), 15);
        t.set_control(2, 0x00);
        t.step(5);
        assert_eq!(t.counter(2), 15);
    }

    #[test]
    fn large_step_handles_multiple_overflows() {
        let mut t = Timers::new();
        t.set_reload(3, 0xFF00);
        t.set_control(3, 0x80 | 0x40);
        assert_eq!(t.step(0x100 * 3 + 5), 0b1000);
        assert_eq!(t.counter(3), 0xFF05);
    }
}
