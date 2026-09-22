//! The four "PSG" channels inherited from the Game Boy: two pulse waves,
//! a programmable wave channel and a noise generator.
//!
//! Each channel produces a 4-bit unsigned level. Periods are expressed in
//! system cycles: the Game Boy Advance clock is four times the Game Boy's,
//! so every classic period constant is multiplied by four.

/// Highest 11-bit frequency value; anything above overflows the channel.
const MAX_FREQUENCY: u16 = 2047;

/// Length counter shared by all channels: counts down at 256 Hz and, when
/// enabled, silences the channel on reaching zero.
#[derive(Debug, Clone, Copy, Default)]
struct Length {
    counter: u16,
    enabled: bool,
}

impl Length {
    /// Loads a fresh count from the register's "sound length" field
    /// (`max - value`).
    fn load(&mut self, value: u16, max: u16) {
        self.counter = max - value;
    }

    /// A trigger with an expired counter restarts it at the maximum.
    fn trigger(&mut self, max: u16) {
        if self.counter == 0 {
            self.counter = max;
        }
    }

    /// The 256 Hz step. Returns `true` when the channel must be silenced.
    fn clock(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            return self.counter == 0;
        }
        false
    }
}

/// Volume envelope: moves the 4-bit volume one step up or down every
/// `period` ticks of the 64 Hz clock.
#[derive(Debug, Clone, Copy, Default)]
struct Envelope {
    initial: u8,
    increase: bool,
    period: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    /// Decodes bits 8–15 of a `SOUNDxCNT` envelope halfword.
    fn set(&mut self, bits: u16) {
        self.period = ((bits >> 8) & 7) as u8;
        self.increase = bits & (1 << 11) != 0;
        self.initial = (bits >> 12) as u8;
    }

    /// Whether the channel's DAC is powered: a zero start volume with a
    /// decreasing envelope switches it off, silencing the channel.
    fn dac_enabled(self) -> bool {
        self.initial != 0 || self.increase
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = self.period;
    }

    /// The 64 Hz step.
    fn clock(&mut self) {
        if self.period == 0 {
            return;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer == 0 {
            self.timer = self.period;
            if self.increase && self.volume < 15 {
                self.volume += 1;
            } else if !self.increase && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

/// Frequency sweep, channel 1 only.
#[derive(Debug, Clone, Copy, Default)]
struct Sweep {
    shift: u8,
    decrease: bool,
    period: u8,
    timer: u8,
    enabled: bool,
    shadow: u16,
}

impl Sweep {
    /// Decodes `SOUND1CNT_L`.
    fn set(&mut self, bits: u16) {
        self.shift = (bits & 7) as u8;
        self.decrease = bits & (1 << 3) != 0;
        self.period = ((bits >> 4) & 7) as u8;
    }

    fn reload_timer(&mut self) {
        // A zero period behaves like eight.
        self.timer = if self.period == 0 { 8 } else { self.period };
    }

    /// The frequency after one sweep step, or `None` on overflow.
    fn next(self) -> Option<u16> {
        let delta = self.shadow >> self.shift;
        let next = if self.decrease {
            self.shadow.wrapping_sub(delta)
        } else {
            self.shadow + delta
        };
        (next <= MAX_FREQUENCY).then_some(next)
    }
}

/// Which eighths of the period a pulse channel spends high.
const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

/// A pulse-wave channel (channels 1 and 2). Channel 2 simply never has
/// its sweep programmed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Square {
    enabled: bool,
    duty: u8,
    frequency: u16,
    /// Cycles until the next duty step.
    timer: u32,
    phase: u8,
    length: Length,
    envelope: Envelope,
    sweep: Sweep,
}

impl Square {
    const LENGTH_MAX: u16 = 64;

    fn period(frequency: u16) -> u32 {
        (2048 - u32::from(frequency)) * 16
    }

    /// `SOUND1CNT_L`.
    pub fn write_sweep(&mut self, value: u16) {
        self.sweep.set(value);
    }

    /// `SOUND1CNT_H` / `SOUND2CNT_L`: length, duty and envelope.
    pub fn write_length_envelope(&mut self, value: u16) {
        self.length.load(value & 0x3F, Self::LENGTH_MAX);
        self.duty = ((value >> 6) & 3) as u8;
        self.envelope.set(value);
        if !self.envelope.dac_enabled() {
            self.enabled = false;
        }
    }

    /// `SOUND1CNT_X` / `SOUND2CNT_H`: frequency, length enable, restart.
    pub fn write_frequency_control(&mut self, value: u16) {
        self.frequency = value & 0x7FF;
        self.length.enabled = value & (1 << 14) != 0;
        if value & (1 << 15) != 0 {
            self.trigger();
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.envelope.dac_enabled();
        self.length.trigger(Self::LENGTH_MAX);
        self.timer = Self::period(self.frequency);
        self.envelope.trigger();
        self.sweep.shadow = self.frequency;
        self.sweep.reload_timer();
        self.sweep.enabled = self.sweep.period != 0 || self.sweep.shift != 0;
        if self.sweep.shift != 0 && self.sweep.next().is_none() {
            self.enabled = false;
        }
    }

    /// Whether the channel is producing sound (`SOUNDCNT_X` status bit).
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled
    }

    /// Advances the waveform by `cycles`.
    pub fn tick(&mut self, cycles: u32) {
        if cycles < self.timer {
            self.timer -= cycles;
            return;
        }
        let period = Self::period(self.frequency);
        let past = cycles - self.timer;
        let steps = 1 + past / period;
        self.timer = period - past % period;
        self.phase = (self.phase + (steps % 8) as u8) % 8;
    }

    pub(super) fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    pub(super) fn clock_envelope(&mut self) {
        self.envelope.clock();
    }

    pub(super) fn clock_sweep(&mut self) {
        let sweep = &mut self.sweep;
        sweep.timer = sweep.timer.saturating_sub(1);
        if sweep.timer != 0 {
            return;
        }
        sweep.reload_timer();
        if !sweep.enabled || sweep.period == 0 {
            return;
        }
        match sweep.next() {
            Some(next) => {
                if sweep.shift != 0 {
                    sweep.shadow = next;
                    self.frequency = next;
                    if sweep.next().is_none() {
                        self.enabled = false;
                    }
                }
            }
            None => self.enabled = false,
        }
    }

    /// The current 4-bit level, or `None` while the DAC is off.
    #[must_use]
    pub fn output(&self) -> Option<u8> {
        if !self.envelope.dac_enabled() {
            return None;
        }
        let high = self.enabled && DUTY[self.duty as usize][self.phase as usize] != 0;
        Some(if high { self.envelope.volume } else { 0 })
    }
}

/// The programmable wave channel (channel 3): 32 or 64 four-bit samples
/// from two 16-byte banks of wave RAM.
#[allow(clippy::struct_excessive_bools)] // mirrors the register bits
#[derive(Debug, Clone, Copy, Default)]
pub struct Wave {
    enabled: bool,
    /// `SOUND3CNT_L` bit 7: the channel's DAC.
    playing: bool,
    two_banks: bool,
    /// Bank that is (or starts) playing; the CPU accesses the other one.
    bank: usize,
    ram: [[u8; 16]; 2],
    position: u8,
    frequency: u16,
    timer: u32,
    volume: u8,
    force_three_quarters: bool,
    length: Length,
}

impl Wave {
    const LENGTH_MAX: u16 = 256;

    fn period(frequency: u16) -> u32 {
        (2048 - u32::from(frequency)) * 8
    }

    /// `SOUND3CNT_L`: bank layout and DAC.
    pub fn write_mode(&mut self, value: u16) {
        self.two_banks = value & (1 << 5) != 0;
        self.bank = usize::from((value >> 6) & 1);
        self.playing = value & (1 << 7) != 0;
        if !self.playing {
            self.enabled = false;
        }
    }

    /// `SOUND3CNT_H`: length and volume.
    pub fn write_length_volume(&mut self, value: u16) {
        self.length.load(value & 0xFF, Self::LENGTH_MAX);
        self.volume = ((value >> 13) & 3) as u8;
        self.force_three_quarters = value & (1 << 15) != 0;
    }

    /// `SOUND3CNT_X`: sample rate, length enable, restart.
    pub fn write_frequency_control(&mut self, value: u16) {
        self.frequency = value & 0x7FF;
        self.length.enabled = value & (1 << 14) != 0;
        if value & (1 << 15) != 0 {
            self.enabled = self.playing;
            self.length.trigger(Self::LENGTH_MAX);
            self.timer = Self::period(self.frequency);
            self.position = 0;
        }
    }

    /// Reads a halfword of wave RAM (`0x90..0xA0`), from the bank that is
    /// not selected for playback.
    #[must_use]
    pub fn read_ram(&self, offset: usize) -> u16 {
        let bank = &self.ram[self.bank ^ 1];
        u16::from_le_bytes([bank[offset & 0xE], bank[(offset & 0xE) + 1]])
    }

    /// Writes a halfword of wave RAM.
    pub fn write_ram(&mut self, offset: usize, value: u16) {
        let bank = &mut self.ram[self.bank ^ 1];
        bank[offset & 0xE..(offset & 0xE) + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Whether the channel is producing sound.
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled
    }

    fn sample_count(&self) -> u8 {
        if self.two_banks { 64 } else { 32 }
    }

    /// Advances playback by `cycles`.
    pub fn tick(&mut self, cycles: u32) {
        if cycles < self.timer {
            self.timer -= cycles;
            return;
        }
        let period = Self::period(self.frequency);
        let past = cycles - self.timer;
        let steps = 1 + past / period;
        self.timer = period - past % period;
        let count = u32::from(self.sample_count());
        self.position = ((u32::from(self.position) + steps) % count) as u8;
    }

    pub(super) fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    /// The current 4-bit level, or `None` while the DAC is off.
    #[must_use]
    pub fn output(&self) -> Option<u8> {
        if !self.playing {
            return None;
        }
        if !self.enabled {
            return Some(0);
        }
        let position = usize::from(self.position);
        let bank = if self.two_banks {
            (self.bank + position / 32) % 2
        } else {
            self.bank
        };
        let byte = self.ram[bank][(position % 32) / 2];
        let sample = if position % 2 == 0 {
            byte >> 4
        } else {
            byte & 0xF
        };
        Some(if self.force_three_quarters {
            sample * 3 / 4
        } else {
            match self.volume {
                0 => 0,
                1 => sample,
                2 => sample >> 1,
                _ => sample >> 2,
            }
        })
    }
}

/// The noise channel (channel 4): a linear-feedback shift register clocked
/// at a programmable rate.
#[derive(Debug, Clone, Copy)]
pub struct Noise {
    enabled: bool,
    lfsr: u16,
    /// 7-bit mode: shorter, more tonal sequence.
    narrow: bool,
    shift: u8,
    ratio: u8,
    timer: u32,
    length: Length,
    envelope: Envelope,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            enabled: false,
            lfsr: 0x7FFF,
            narrow: false,
            shift: 0,
            ratio: 0,
            timer: 0,
            length: Length::default(),
            envelope: Envelope::default(),
        }
    }
}

impl Noise {
    const LENGTH_MAX: u16 = 64;

    fn period(&self) -> u32 {
        let divisor = if self.ratio == 0 {
            16
        } else {
            32 * u32::from(self.ratio)
        };
        divisor << (self.shift + 1)
    }

    /// `SOUND4CNT_L`: length and envelope.
    pub fn write_length_envelope(&mut self, value: u16) {
        self.length.load(value & 0x3F, Self::LENGTH_MAX);
        self.envelope.set(value);
        if !self.envelope.dac_enabled() {
            self.enabled = false;
        }
    }

    /// `SOUND4CNT_H`: clock divider, width, length enable, restart.
    pub fn write_control(&mut self, value: u16) {
        self.ratio = (value & 7) as u8;
        self.narrow = value & (1 << 3) != 0;
        self.shift = ((value >> 4) & 0xF) as u8;
        self.length.enabled = value & (1 << 14) != 0;
        if value & (1 << 15) != 0 {
            self.enabled = self.envelope.dac_enabled();
            self.length.trigger(Self::LENGTH_MAX);
            self.timer = self.period();
            self.envelope.trigger();
            self.lfsr = 0x7FFF;
        }
    }

    /// Whether the channel is producing sound.
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled
    }

    /// Advances the shift register by `cycles`.
    pub fn tick(&mut self, cycles: u32) {
        // Shift values 14 and 15 are too slow to be usable; the sequence
        // simply stops.
        if self.shift >= 14 {
            return;
        }
        let mut cycles = cycles;
        while cycles >= self.timer {
            cycles -= self.timer;
            self.timer = self.period();
            let feedback = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr = (self.lfsr >> 1) | (feedback << 14);
            if self.narrow {
                self.lfsr = (self.lfsr & !(1 << 6)) | (feedback << 6);
            }
        }
        self.timer -= cycles;
    }

    pub(super) fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    pub(super) fn clock_envelope(&mut self) {
        self.envelope.clock();
    }

    /// The current 4-bit level, or `None` while the DAC is off.
    #[must_use]
    pub fn output(&self) -> Option<u8> {
        if !self.envelope.dac_enabled() {
            return None;
        }
        let high = self.enabled && self.lfsr & 1 == 0;
        Some(if high { self.envelope.volume } else { 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_follows_its_duty_cycle() {
        let mut sq = Square::default();
        // 50% duty, full volume, no envelope.
        sq.write_length_envelope(0xF000 | (2 << 6));
        sq.write_frequency_control(0x8000 | 2047); // period 16 cycles
        assert!(sq.active());
        let mut levels = Vec::new();
        for _ in 0..8 {
            levels.push(sq.output().unwrap());
            sq.tick(16);
        }
        assert_eq!(levels, [15, 0, 0, 0, 0, 15, 15, 15]);
        // Big jumps land on the right phase.
        sq.tick(16 * 8 * 3 + 16);
        assert_eq!(sq.output(), Some(0));
    }

    #[test]
    fn square_dac_off_and_length_expiry() {
        let mut sq = Square::default();
        sq.write_length_envelope(0);
        sq.write_frequency_control(0x8000);
        assert_eq!(sq.output(), None, "DAC off");
        assert!(!sq.active());

        // Length 62 -> counter 2, length enable: silent after two clocks.
        sq.write_length_envelope(0xF000 | 0x3E); // length 62
        sq.write_frequency_control(0x8000 | 0x4000);
        assert!(sq.active());
        sq.clock_length();
        assert!(sq.active());
        sq.clock_length();
        assert!(!sq.active());
        assert_eq!(sq.output(), Some(0), "DAC still on, channel silent");
    }

    #[test]
    fn envelope_steps_volume() {
        let mut sq = Square::default();
        // Start at 3, increasing, period 2.
        sq.write_length_envelope(0x3000 | (1 << 11) | (2 << 8) | (3 << 6));
        sq.write_frequency_control(0x8000);
        assert_eq!(sq.envelope.volume, 3);
        sq.clock_envelope();
        assert_eq!(sq.envelope.volume, 3);
        sq.clock_envelope();
        assert_eq!(sq.envelope.volume, 4);
        for _ in 0..40 {
            sq.clock_envelope();
        }
        assert_eq!(sq.envelope.volume, 15, "saturates");
    }

    #[test]
    fn sweep_raises_frequency_and_overflows() {
        let mut sq = Square::default();
        sq.write_sweep((1 << 4) | 1); // period 1, shift 1, increase
        sq.write_length_envelope(0xF000);
        sq.write_frequency_control(0x8000 | 512);
        sq.clock_sweep();
        assert_eq!(sq.frequency, 512 + 256);
        assert!(sq.active());
        sq.clock_sweep();
        assert_eq!(sq.frequency, 768 + 384);
        assert!(sq.active());
        sq.clock_sweep();
        // 1152 + 576 = 1728 fits, but the following step would not: the
        // channel switches off with the last valid frequency.
        assert_eq!(sq.frequency, 1728);
        assert!(!sq.active());
    }

    #[test]
    fn wave_plays_ram_from_the_selected_bank() {
        let mut wave = Wave::default();
        // Bank 1 selected for playback: CPU writes go to bank 0. Write a
        // ramp there, then switch playback to bank 0.
        wave.write_mode(1 << 6);
        for i in 0..8u16 {
            wave.write_ram(usize::from(i * 2), 0xF0F0 - i * 0x1010);
        }
        assert_eq!(wave.read_ram(2), 0xE0E0);
        wave.write_mode(1 << 7); // play, bank 0
        assert_eq!(wave.read_ram(0), 0, "bank 1 is what the CPU sees now");
        wave.write_length_volume(1 << 13); // 100%
        wave.write_frequency_control(0x8000 | 2047); // period 8
        let mut levels = Vec::new();
        for _ in 0..4 {
            levels.push(wave.output().unwrap());
            wave.tick(8);
        }
        // Bytes are F0, F0, E0, E0 ... high nibble first.
        assert_eq!(levels, [15, 0, 15, 0]);
        // Now at sample 4: the high nibble of the third byte, 0xE.
        assert_eq!(wave.output(), Some(14));
        wave.write_length_volume(2 << 13); // 50%
        assert_eq!(wave.output(), Some(7));
        wave.write_length_volume((1 << 13) | (1 << 15)); // forced 75%
        assert_eq!(wave.output(), Some(10));
        wave.write_mode(0);
        assert_eq!(wave.output(), None, "DAC off");
    }

    #[test]
    fn noise_shift_register_sequences() {
        let mut noise = Noise::default();
        noise.write_length_envelope(0xF000);
        noise.write_control(0x8000 | (1 << 3)); // 7-bit, ratio 0, shift 0
        assert_eq!(noise.period(), 32);
        assert_eq!(noise.output(), Some(0), "all ones: bit 0 set");
        // The 7-bit sequence repeats every 127 steps.
        let start = noise.lfsr & 0x7F;
        noise.tick(32 * 127);
        assert_eq!(noise.lfsr & 0x7F, start);
        noise.tick(32);
        assert_ne!(noise.lfsr & 0x7F, start);
        noise.write_control(0x8000 | (14 << 4));
        let frozen = noise.lfsr;
        noise.tick(1 << 20);
        assert_eq!(noise.lfsr, frozen, "shift 14 stops the clock");
    }
}
