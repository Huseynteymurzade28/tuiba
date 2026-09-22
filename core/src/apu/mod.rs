//! Sound: the four PSG channels, the two direct-sound FIFOs, the mixer
//! and the sample buffer.
//!
//! The APU is stepped with the rest of the system and emits one stereo
//! sample pair every [`CYCLES_PER_SAMPLE`] cycles, i.e. at
//! [`SAMPLE_RATE`] Hz, into a buffer the frontend drains.
//!
//! Channel timers are advanced once per output sample rather than per
//! system cycle. That is exact for the sample values (a channel's level at
//! the sampling instant is what is heard) and keeps the cost proportional
//! to the sample rate.

pub mod fifo;
pub mod psg;

use fifo::Fifo;
use psg::{Noise, Square, Wave};

/// Output sample rate in Hz. Chosen so that a sample is an integer number
/// of system cycles (16 777 216 / 32 768 = 512).
pub const SAMPLE_RATE: u32 = 32_768;
/// System cycles per output sample.
pub const CYCLES_PER_SAMPLE: u32 = 512;
/// The frame sequencer runs at 512 Hz: every 64 samples.
const SEQUENCER_INTERVAL: u32 = SAMPLE_RATE / 512;

/// First and last register offsets the APU owns.
pub const REGISTER_RANGE: std::ops::RangeInclusive<u32> = 0x060..=0x0A7;
/// Register offsets relative to the I/O base address.
#[allow(missing_docs)]
pub mod reg {
    pub const SOUND1CNT_L: u32 = 0x060;
    pub const SOUND1CNT_H: u32 = 0x062;
    pub const SOUND1CNT_X: u32 = 0x064;
    pub const SOUND2CNT_L: u32 = 0x068;
    pub const SOUND2CNT_H: u32 = 0x06C;
    pub const SOUND3CNT_L: u32 = 0x070;
    pub const SOUND3CNT_H: u32 = 0x072;
    pub const SOUND3CNT_X: u32 = 0x074;
    pub const SOUND4CNT_L: u32 = 0x078;
    pub const SOUND4CNT_H: u32 = 0x07C;
    pub const SOUNDCNT_L: u32 = 0x080;
    pub const SOUNDCNT_H: u32 = 0x082;
    pub const SOUNDCNT_X: u32 = 0x084;
    pub const SOUNDBIAS: u32 = 0x088;
    pub const WAVE_RAM: u32 = 0x090;
    pub const FIFO_A: u32 = 0x0A0;
    pub const FIFO_B: u32 = 0x0A4;
}

/// Number of halfword registers from `SOUND1CNT_L` up to wave RAM.
const REGISTER_COUNT: usize = ((reg::WAVE_RAM - reg::SOUND1CNT_L) / 2) as usize;

/// Bits that read back from each register in `0x60..0x90`; write-only
/// fields (lengths, frequencies, restart bits) and unused registers read
/// as zero.
const READ_MASKS: [u16; REGISTER_COUNT] = [
    0x007F, // 60 SOUND1CNT_L
    0xFFC0, // 62 SOUND1CNT_H
    0x4000, // 64 SOUND1CNT_X
    0x0000, // 66
    0xFFC0, // 68 SOUND2CNT_L
    0x0000, // 6A
    0x4000, // 6C SOUND2CNT_H
    0x0000, // 6E
    0x00E0, // 70 SOUND3CNT_L
    0xE000, // 72 SOUND3CNT_H
    0x4000, // 74 SOUND3CNT_X
    0x0000, // 76
    0xFF00, // 78 SOUND4CNT_L
    0x0000, // 7A
    0x40FF, // 7C SOUND4CNT_H
    0x0000, // 7E
    0xFF77, // 80 SOUNDCNT_L
    0x770F, // 82 SOUNDCNT_H
    0x0080, // 84 SOUNDCNT_X (status bits are added live)
    0x0000, // 86
    0xC3FE, // 88 SOUNDBIAS
    0x0000, // 8A
    0x0000, // 8C
    0x0000, // 8E
];

/// Full-scale of the mixed 10-bit output, before the bias is added.
const OUTPUT_LIMIT: i32 = 0x200;

/// The sound unit.
#[derive(Debug, Clone)]
pub struct Apu {
    /// Registers as last written, for read-back and byte-wise merging.
    raw: [u16; REGISTER_COUNT],
    square1: Square,
    square2: Square,
    wave: Wave,
    noise: Noise,
    /// Direct-sound FIFOs A and B.
    fifo: [Fifo; 2],
    /// Cycles accumulated towards the next output sample.
    cycles: u32,
    /// Samples produced since the last frame-sequencer step.
    sequencer_samples: u32,
    sequencer_step: u8,
    /// Interleaved stereo output, left then right.
    samples: Vec<i16>,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    /// The sound unit in its power-on state.
    #[must_use]
    pub fn new() -> Self {
        let mut apu = Self {
            raw: [0; REGISTER_COUNT],
            square1: Square::default(),
            square2: Square::default(),
            wave: Wave::default(),
            noise: Noise::default(),
            fifo: [Fifo::new(); 2],
            cycles: 0,
            sequencer_samples: 0,
            sequencer_step: 0,
            samples: Vec::new(),
        };
        apu.raw[Self::index(reg::SOUNDBIAS)] = 0x0200;
        apu
    }

    fn index(offset: u32) -> usize {
        ((offset - reg::SOUND1CNT_L) / 2) as usize
    }

    /// `SOUNDCNT_X` bit 7.
    fn master_enabled(&self) -> bool {
        self.raw[Self::index(reg::SOUNDCNT_X)] & 0x80 != 0
    }

    /// Reads a halfword register; `offset` must be within
    /// [`REGISTER_RANGE`] and even.
    #[must_use]
    pub fn read16(&self, offset: u32) -> u16 {
        match offset {
            reg::SOUNDCNT_X => {
                let status = u16::from(self.square1.active())
                    | u16::from(self.square2.active()) << 1
                    | u16::from(self.wave.active()) << 2
                    | u16::from(self.noise.active()) << 3;
                self.raw[Self::index(offset)] & 0x80 | status
            }
            reg::SOUND1CNT_L..reg::WAVE_RAM => {
                let i = Self::index(offset);
                self.raw[i] & READ_MASKS[i]
            }
            reg::WAVE_RAM..reg::FIFO_A => self.wave.read_ram((offset - reg::WAVE_RAM) as usize),
            _ => 0,
        }
    }

    /// Writes a halfword register.
    pub fn write16(&mut self, offset: u32, value: u16) {
        match offset {
            reg::SOUND1CNT_L..reg::SOUNDCNT_H => {
                // The channel registers are frozen while the master enable
                // is off.
                if !self.master_enabled() {
                    return;
                }
                self.raw[Self::index(offset)] = value;
                self.write_channel_register(offset, value);
            }
            reg::SOUNDCNT_H => {
                // Bits 11 and 15 reset the FIFOs and read back as zero.
                if value & (1 << 11) != 0 {
                    self.fifo[0].reset();
                }
                if value & (1 << 15) != 0 {
                    self.fifo[1].reset();
                }
                self.raw[Self::index(offset)] = value & !0x8800;
            }
            reg::SOUNDBIAS => self.raw[Self::index(offset)] = value,
            reg::SOUNDCNT_X => {
                let was_enabled = self.master_enabled();
                self.raw[Self::index(offset)] = value & 0x80;
                if was_enabled && !self.master_enabled() {
                    self.power_off();
                }
            }
            reg::WAVE_RAM..reg::FIFO_A => self
                .wave
                .write_ram((offset - reg::WAVE_RAM) as usize, value),
            reg::FIFO_A..=0x0A7 => self.fifo_for(offset).push(&value.to_le_bytes()),
            _ => {}
        }
    }

    fn fifo_for(&mut self, offset: u32) -> &mut Fifo {
        &mut self.fifo[usize::from(offset >= reg::FIFO_B)]
    }

    /// Writes a byte register by merging it into the last written halfword
    /// (not the readable value, whose write-only fields read as zero).
    pub fn write8(&mut self, offset: u32, value: u8) {
        if offset >= reg::FIFO_A {
            self.fifo_for(offset).push(&[value]);
            return;
        }
        let aligned = offset & !1;
        let shift = (offset & 1) * 8;
        let current = match aligned {
            reg::SOUND1CNT_L..reg::WAVE_RAM => self.raw[Self::index(aligned)],
            _ => self.read16(aligned),
        };
        let merged = (current & !(0xFF << shift)) | (u16::from(value) << shift);
        self.write16(aligned, merged);
    }

    fn write_channel_register(&mut self, offset: u32, value: u16) {
        match offset {
            reg::SOUND1CNT_L => self.square1.write_sweep(value),
            reg::SOUND1CNT_H => self.square1.write_length_envelope(value),
            reg::SOUND1CNT_X => self.square1.write_frequency_control(value),
            reg::SOUND2CNT_L => self.square2.write_length_envelope(value),
            reg::SOUND2CNT_H => self.square2.write_frequency_control(value),
            reg::SOUND3CNT_L => self.wave.write_mode(value),
            reg::SOUND3CNT_H => self.wave.write_length_volume(value),
            reg::SOUND3CNT_X => self.wave.write_frequency_control(value),
            reg::SOUND4CNT_L => self.noise.write_length_envelope(value),
            reg::SOUND4CNT_H => self.noise.write_control(value),
            _ => {}
        }
    }

    /// Clearing the master enable resets every PSG register and channel.
    fn power_off(&mut self) {
        for i in Self::index(reg::SOUND1CNT_L)..Self::index(reg::SOUNDCNT_H) {
            self.raw[i] = 0;
        }
        self.square1 = Square::default();
        self.square2 = Square::default();
        self.wave = Wave::default();
        self.noise = Noise::default();
        self.sequencer_step = 0;
        self.sequencer_samples = 0;
    }

    /// Advances the sound unit by `cycles` system cycles, producing
    /// output samples as their instants pass.
    pub fn step(&mut self, cycles: u32) {
        self.cycles += cycles;
        while self.cycles >= CYCLES_PER_SAMPLE {
            self.cycles -= CYCLES_PER_SAMPLE;
            self.sample();
        }
    }

    /// The samples produced so far (interleaved stereo, left first).
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// Discards the buffered samples, typically after copying them out.
    pub fn clear_samples(&mut self) {
        self.samples.clear();
    }

    /// Timer `timer` (0 or 1) overflowed `count` times: every FIFO clocked
    /// by it advances that many samples. Returns a bit mask (bit 0 = A,
    /// bit 1 = B) of the FIFOs that now want a DMA refill.
    pub fn timer_overflow(&mut self, timer: usize, count: u32) -> u8 {
        let cnt_h = self.raw[Self::index(reg::SOUNDCNT_H)];
        let mut refill = 0;
        for (n, fifo) in self.fifo.iter_mut().enumerate() {
            let selected = usize::from((cnt_h >> (10 + 4 * n)) & 1);
            if selected != timer {
                continue;
            }
            for _ in 0..count {
                if fifo.pop() {
                    refill |= 1 << n;
                }
            }
        }
        refill
    }

    fn sample(&mut self) {
        if self.master_enabled() {
            self.square1.tick(CYCLES_PER_SAMPLE);
            self.square2.tick(CYCLES_PER_SAMPLE);
            self.wave.tick(CYCLES_PER_SAMPLE);
            self.noise.tick(CYCLES_PER_SAMPLE);
            self.sequencer_samples += 1;
            if self.sequencer_samples == SEQUENCER_INTERVAL {
                self.sequencer_samples = 0;
                self.clock_sequencer();
            }
        }
        let (left, right) = self.mix();
        self.samples.push(left);
        self.samples.push(right);
    }

    /// One 512 Hz step: lengths on even steps, sweep on 2 and 6, envelopes
    /// on 7.
    fn clock_sequencer(&mut self) {
        let step = self.sequencer_step;
        self.sequencer_step = (step + 1) % 8;
        if step.is_multiple_of(2) {
            self.square1.clock_length();
            self.square2.clock_length();
            self.wave.clock_length();
            self.noise.clock_length();
        }
        if step == 2 || step == 6 {
            self.square1.clock_sweep();
        }
        if step == 7 {
            self.square1.clock_envelope();
            self.square2.clock_envelope();
            self.noise.clock_envelope();
        }
    }

    /// Mixes the channels into a `(left, right)` pair.
    fn mix(&self) -> (i16, i16) {
        if !self.master_enabled() {
            return (0, 0);
        }
        let cnt_l = self.raw[Self::index(reg::SOUNDCNT_L)];
        let cnt_h = self.raw[Self::index(reg::SOUNDCNT_H)];
        // Each channel's DAC output is centred: level 0..15 -> -15..15.
        let levels = [
            self.square1.output(),
            self.square2.output(),
            self.wave.output(),
            self.noise.output(),
        ]
        .map(|level| level.map_or(0, |v| i32::from(v) * 2 - 15));
        // SOUNDCNT_H bits 1:0 scale the PSG mix to 25/50/100 %.
        let ratio = match cnt_h & 3 {
            0 => 1,
            1 => 2,
            _ => 4,
        };
        // Direct sound: an 8-bit sample at 50 % or 100 %, where 100 %
        // spans the full 10-bit range.
        let direct = [0, 1].map(|n| {
            let full = cnt_h & (1 << (2 + n)) != 0;
            i32::from(self.fifo[n].sample()) * if full { 4 } else { 2 }
        });
        // `side` is called with the bit positions of a side's PSG enables
        // and volume, and of the FIFO A/B enables for the same side.
        let side = |enable_shift: u32, volume_shift: u32, direct_shift: u32| {
            let enables = cnt_l >> enable_shift;
            let volume = i32::from((cnt_l >> volume_shift) & 7) + 1;
            let sum: i32 = levels
                .iter()
                .enumerate()
                .filter(|(n, _)| enables & (1 << n) != 0)
                .map(|(_, level)| level)
                .sum();
            // Four channels at full volume and 100 % ratio span the whole
            // 10-bit range: 60 * 8 * 4 = 1920 -> 512.
            let psg = sum * volume * ratio * OUTPUT_LIMIT / 1920;
            let dma: i32 = direct
                .iter()
                .enumerate()
                .filter(|(n, _)| cnt_h & (1 << (direct_shift + 4 * *n as u32)) != 0)
                .map(|(_, sample)| sample)
                .sum();
            let total = (psg + dma).clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT - 1);
            (total << 6) as i16
        };
        (side(12, 4, 9), side(8, 0, 8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn powered() -> Apu {
        let mut apu = Apu::new();
        apu.write16(reg::SOUNDCNT_X, 0x80);
        apu
    }

    #[test]
    fn registers_read_back_through_masks() {
        let mut apu = powered();
        apu.write16(reg::SOUND1CNT_H, 0xFFFF);
        assert_eq!(apu.read16(reg::SOUND1CNT_H), 0xFFC0, "length is write-only");
        apu.write16(reg::SOUND1CNT_X, 0xFFFF);
        assert_eq!(apu.read16(reg::SOUND1CNT_X), 0x4000);
        assert_eq!(apu.read16(reg::SOUNDBIAS), 0x0200, "power-on bias");
        apu.write16(0x066, 0x1234);
        assert_eq!(apu.read16(0x066), 0, "unused register");
        apu.write16(reg::FIFO_A, 0x1234);
        assert_eq!(apu.read16(reg::FIFO_A), 0, "FIFO is write-only");
    }

    #[test]
    fn byte_writes_merge_with_write_only_fields_intact() {
        let mut apu = powered();
        // Length 10, duty 3, volume 15.
        apu.write16(reg::SOUND1CNT_H, 0xF000 | (3 << 6) | 0x0A);
        // Rewriting the high byte alone must not clobber the length.
        apu.write8(reg::SOUND1CNT_H + 1, 0x80);
        assert_eq!(
            apu.raw[Apu::index(reg::SOUND1CNT_H)],
            0x8000 | (3 << 6) | 0x0A
        );
        assert_eq!(apu.read16(reg::SOUND1CNT_H), 0x80C0);
    }

    #[test]
    fn status_bits_track_active_channels() {
        let mut apu = powered();
        assert_eq!(apu.read16(reg::SOUNDCNT_X), 0x80);
        apu.write16(reg::SOUND2CNT_L, 0xF000);
        apu.write16(reg::SOUND2CNT_H, 0x8000 | 0x03E8);
        assert_eq!(apu.read16(reg::SOUNDCNT_X), 0x82);
        apu.write16(reg::SOUND4CNT_L, 0xF000);
        apu.write16(reg::SOUND4CNT_H, 0x8000);
        assert_eq!(apu.read16(reg::SOUNDCNT_X), 0x8A);
    }

    #[test]
    fn master_disable_resets_and_freezes_psg_registers() {
        let mut apu = powered();
        apu.write16(reg::SOUND1CNT_L, 0x0077);
        apu.write16(reg::SOUNDCNT_L, 0xFF77);
        apu.write16(reg::SOUNDCNT_H, 0x0002);
        apu.write16(reg::SOUNDCNT_X, 0);
        assert_eq!(apu.read16(reg::SOUND1CNT_L), 0);
        assert_eq!(apu.read16(reg::SOUNDCNT_L), 0);
        assert_eq!(apu.read16(reg::SOUNDCNT_H), 0x0002, "not a PSG register");
        apu.write16(reg::SOUND1CNT_L, 0x0077);
        assert_eq!(apu.read16(reg::SOUND1CNT_L), 0, "writes ignored while off");
        apu.step(CYCLES_PER_SAMPLE * 4);
        assert!(apu.samples().iter().all(|&s| s == 0), "silence while off");
    }

    #[test]
    fn produces_one_stereo_pair_per_512_cycles() {
        let mut apu = powered();
        apu.step(511);
        assert!(apu.samples().is_empty());
        apu.step(1);
        assert_eq!(apu.samples().len(), 2);
        apu.step(CYCLES_PER_SAMPLE * 10 + 3);
        assert_eq!(apu.samples().len(), 22);
        apu.clear_samples();
        assert!(apu.samples().is_empty());
    }

    #[test]
    fn square_wave_reaches_the_output_with_panning() {
        let mut apu = powered();
        // Channel 2 only on the left, full master volume, 100 % ratio.
        apu.write16(reg::SOUNDCNT_L, (1 << 13) | 0x77);
        apu.write16(reg::SOUNDCNT_H, 2);
        apu.write16(reg::SOUND2CNT_L, 0xF000 | (2 << 6)); // 50 % duty
        apu.write16(reg::SOUND2CNT_H, 0x8000 | 1024); // period 16 384 cycles
        // 32 samples per duty step: sample the first quarter of the wave.
        apu.step(CYCLES_PER_SAMPLE * 64);
        let samples = apu.samples();
        let left: Vec<i16> = samples.iter().step_by(2).copied().collect();
        let right: Vec<i16> = samples.iter().skip(1).step_by(2).copied().collect();
        assert!(right.iter().all(|&s| s == 0), "not routed right");
        // Level 15 -> +15 * 8 * 4 * 512 / 1920 = 128 -> << 6.
        assert_eq!(left[0], 128 << 6);
        // After the first eighth the 50 % duty drops low: -15 -> -128.
        assert_eq!(left[40], -128 << 6);
    }

    #[test]
    fn fifo_samples_follow_their_timer_and_panning() {
        let mut apu = powered();
        // FIFO A: 100 %, timer 0, left only. FIFO B: 50 %, timer 1, right.
        apu.write16(reg::SOUNDCNT_H, (1 << 2) | (1 << 9) | (1 << 14) | (1 << 12));
        apu.write16(reg::FIFO_A, 0x0040); // bytes 0x40, 0x00
        apu.write16(reg::FIFO_A + 2, 0x0000);
        apu.write8(reg::FIFO_B, 0x80); // -128
        assert_eq!(apu.read16(reg::SOUNDCNT_H) & 0x8800, 0, "reset bits");
        assert_eq!(apu.timer_overflow(0, 1), 0b01, "A wants a refill");
        assert_eq!(apu.timer_overflow(1, 1), 0b10);
        assert_eq!(apu.fifo[0].sample(), 0x40);
        assert_eq!(apu.fifo[1].sample(), -128);
        assert_eq!(apu.mix(), ((0x40 * 4) << 6, (-128 * 2) << 6));
        // Timer 0 does not clock FIFO B, and A moves on to its next byte.
        apu.timer_overflow(0, 1);
        assert_eq!(apu.fifo[1].sample(), -128);
        assert_eq!(apu.mix(), (0, (-128 * 2) << 6));
        // Reset bit empties A and silences it.
        apu.write16(reg::SOUNDCNT_H, (1 << 11) | (1 << 9));
        assert!(apu.fifo[0].is_empty());
        apu.timer_overflow(0, 1);
        assert_eq!(apu.mix(), (0, 0));
    }

    #[test]
    fn length_counter_silences_through_the_sequencer() {
        let mut apu = powered();
        apu.write16(reg::SOUNDCNT_L, 0xFF77);
        apu.write16(reg::SOUNDCNT_H, 2);
        // Length field 63 -> one 256 Hz tick, length enabled.
        apu.write16(reg::SOUND1CNT_H, 0xF000 | 63);
        apu.write16(reg::SOUND1CNT_X, 0x8000 | 0x4000 | 1024);
        assert!(apu.square1.active());
        // Length clocks on sequencer step 0: the first fires after 64
        // samples.
        apu.step(CYCLES_PER_SAMPLE * SEQUENCER_INTERVAL);
        assert!(!apu.square1.active());
        assert_eq!(apu.read16(reg::SOUNDCNT_X) & 1, 0);
    }
}
