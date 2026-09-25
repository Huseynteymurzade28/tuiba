//! Sound output through the default audio device.
//!
//! The emulator produces samples at its own rate (32 768 Hz, see
//! [`tuiba_core::apu`]) in bursts of one frame; the device asks for them
//! at its own rate in bursts of its own size. A queue sits between the
//! two, and the device callback resamples linearly out of it. Latency is
//! the queue depth, held near [`TARGET`] by resampling a touch faster or
//! slower as the queue fills or drains (at most [`MAX_RATE_SKEW`], far
//! below what an ear notices as pitch). The two clocks never agree
//! exactly, and without this the difference ends in the queue running dry
//! or being trimmed, either of which is a click.
//!
//! A queue that runs dry anyway (the game fell behind: a slow terminal, a
//! busy machine) is not played from again until it holds [`PRIME`]
//! frames. Playing each frame the moment it arrives would alternate sound
//! and silence every few milliseconds, which is heard as a loud buzz; a
//! short gap is much kinder.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use tuiba_core::apu::SAMPLE_RATE;

use crate::stats::AudioStats;

/// A left/right pair at the emulator's rate.
type Frame = [i16; 2];

/// Silence queued before the first sample, so a late frame does not
/// starve the device straight away (~60 ms).
const PRIME: usize = 2048;
/// Depth the queue is trimmed back to (~100 ms).
const TARGET: usize = 3277;
/// Depth past which the queue is trimmed (~250 ms). Only a stall (the
/// device stopped asking for a while) gets this far; drift is taken care
/// of by the rate control.
const HIGH_WATER: usize = 8192;
/// Largest change to the resampling rate, as a fraction of it: 0.5 %,
/// about a twelfth of a semitone.
const MAX_RATE_SKEW: f64 = 0.005;
/// Weight of the newest queue depth in its running average. The queue
/// fills a frame at a time and drains a device buffer at a time, so a
/// single reading swings by hundreds of samples.
const DEPTH_SMOOTHING: f64 = 0.05;

/// [`TARGET`] as a float, for the rate control.
#[allow(clippy::cast_precision_loss)] // 3277 is exact in an f64
const TARGET_DEPTH: f64 = TARGET as f64;

/// What the emulator side and the device callback share.
#[derive(Debug)]
struct Shared {
    queue: VecDeque<Frame>,
    /// The queue ran dry and has not been refilled to [`PRIME`] since.
    starved: bool,
    /// Times the queue has run dry, for `--stats`.
    dry_spells: u32,
}

impl Shared {
    fn primed() -> Self {
        Self {
            queue: VecDeque::from(vec![[0, 0]; PRIME]),
            starved: false,
            dry_spells: 0,
        }
    }

    /// Hands `emit` the next `count` frames at the device's rate: from
    /// the queue while it lasts, silence once it has run dry and until it
    /// holds [`PRIME`] frames again.
    fn play(&mut self, resampler: &mut Resampler, count: usize, mut emit: impl FnMut(Frame)) {
        if self.starved && self.queue.len() >= PRIME {
            self.starved = false;
            resampler.resume();
        }
        resampler.track(self.queue.len());
        for _ in 0..count {
            let frame = if self.starved {
                None
            } else {
                resampler.next(&mut self.queue)
            };
            if frame.is_none() && !self.starved {
                self.dry_spells += 1;
            }
            self.starved = frame.is_none();
            emit(frame.unwrap_or([0, 0]));
        }
    }
}

/// Why no sound could be set up.
#[derive(Debug)]
pub struct AudioError(String);

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AudioError {}

/// An open output stream and the queue feeding it.
pub struct AudioOutput {
    shared: Arc<Mutex<Shared>>,
    /// Dropping the stream stops playback.
    _stream: cpal::Stream,
}

impl fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioOutput").finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// Opens the default output device in its default configuration.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no device or it refuses a stream.
    pub fn open() -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| AudioError("no output device".into()))?;
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError(e.to_string()))?;
        let config = supported.config();
        let shared = Arc::new(Mutex::new(Shared::primed()));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, &shared),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, &shared),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, &shared),
            _ => build::<f32>(&device, &config, &shared),
        }
        .map_err(|e| AudioError(e.to_string()))?;
        stream.play().map_err(|e| AudioError(e.to_string()))?;
        Ok(Self {
            shared,
            _stream: stream,
        })
    }

    /// Queues interleaved stereo samples at the emulator's rate, scaled
    /// to `volume` percent.
    pub fn push(&self, samples: &[i16], volume: u8) {
        let volume = i32::from(volume.min(100));
        let scale = |s: i16| (i32::from(s) * volume / 100) as i16;
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let queue = &mut shared.queue;
        queue.extend(
            samples
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[l, r]| [scale(l), scale(r)]),
        );
        if queue.len() > HIGH_WATER {
            let excess = queue.len() - TARGET;
            queue.drain(..excess);
        }
    }

    /// Drops what is queued and primes silence again, for a jump in time
    /// the queued samples do not belong to (loading a save state). The
    /// silence is what [`AudioOutput::open`] starts with: without it the
    /// device would run dry before the first frame after the jump.
    pub fn flush(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        *shared = Shared {
            dry_spells: shared.dry_spells,
            ..Shared::primed()
        };
    }

    /// How much sound is queued and how often the queue ran dry.
    pub fn stats(&self) -> AudioStats {
        let shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let frames = u32::try_from(shared.queue.len()).unwrap_or(u32::MAX);
        AudioStats {
            queued_ms: frames.saturating_mul(1000) / SAMPLE_RATE,
            dry_spells: shared.dry_spells,
        }
    }
}

/// Builds the output stream for sample type `T`, with the resampling
/// callback closed over its own state.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Mutex<Shared>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<i16>,
{
    let channels = usize::from(config.channels).max(1);
    let mut resampler = Resampler::new(SAMPLE_RATE, config.sample_rate);
    let shared = Arc::clone(shared);
    device.build_output_stream(
        *config,
        move |out: &mut [T], _| {
            let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
            let mut frames = out.chunks_mut(channels);
            shared.play(&mut resampler, frames.len(), |[left, right]| {
                let frame = frames.next().expect("one output frame per frame played");
                // Stereo goes to the first two channels; a mono device
                // gets the left, extra channels are silent.
                frame[0] = T::from_sample(left);
                if let Some(slot) = frame.get_mut(1) {
                    *slot = T::from_sample(right);
                }
                for slot in frame.iter_mut().skip(2) {
                    *slot = T::from_sample(0);
                }
            });
        },
        |_| {}, // Underruns and device hiccups are not worth interrupting a game for.
        None,
    )
}

/// Linear interpolation from the emulator's rate to the device's.
struct Resampler {
    /// Input frames per output frame, as the two nominal rates say.
    nominal: f64,
    /// The same, skewed by the rate control.
    step: f64,
    /// Running average of the queue depth.
    depth: f64,
    /// Position between `prev` (0) and `next` (1).
    phase: f64,
    prev: Frame,
    next: Frame,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        let nominal = f64::from(from) / f64::from(to.max(1));
        Self {
            nominal,
            step: nominal,
            depth: TARGET_DEPTH,
            phase: 0.0,
            prev: [0, 0],
            next: [0, 0],
        }
    }

    /// Steers the rate by the queue depth: a queue above [`TARGET`] is
    /// played slightly faster, one below it slightly slower.
    #[allow(clippy::cast_precision_loss)] // a queue is a few thousand frames deep
    fn track(&mut self, depth: usize) {
        self.depth += (depth as f64 - self.depth) * DEPTH_SMOOTHING;
        let error = ((self.depth - TARGET_DEPTH) / TARGET_DEPTH).clamp(-1.0, 1.0);
        self.step = self.nominal * (1.0 + error * MAX_RATE_SKEW);
    }

    /// Starts over from silence after the queue ran dry, so the first
    /// output frame does not interpolate from a sound long gone.
    fn resume(&mut self) {
        self.phase = 0.0;
        self.prev = [0, 0];
        self.next = [0, 0];
    }

    /// The next output frame, or `None` when the queue ran dry.
    fn next(&mut self, queue: &mut VecDeque<Frame>) -> Option<Frame> {
        self.phase += self.step;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.prev = self.next;
            self.next = queue.pop_front()?;
        }
        let mix = |a: i16, b: i16| {
            let t = self.phase;
            (f64::from(a) * (1.0 - t) + f64::from(b) * t) as i16
        };
        Some([
            mix(self.prev[0], self.next[0]),
            mix(self.prev[1], self.next[1]),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_interpolates_and_reports_a_dry_queue() {
        // 2:1 downsample: every output frame consumes two inputs, and the
        // output trails the input by one frame (the interpolation window).
        let mut r = Resampler::new(2, 1);
        let mut q: VecDeque<Frame> = [[0, 0], [100, -100], [200, -200], [300, -300]].into();
        assert_eq!(r.next(&mut q), Some([0, 0]));
        assert_eq!(r.next(&mut q), Some([200, -200]));
        assert_eq!(r.next(&mut q), None, "dry");

        // 1:2 upsample: a point halfway between neighbours.
        let mut r = Resampler::new(1, 2);
        let mut q: VecDeque<Frame> = [[0, 0], [100, 100]].into();
        let out: Vec<_> = (0..5).map(|_| r.next(&mut q).unwrap()).collect();
        assert_eq!(out, [[0, 0], [0, 0], [0, 0], [0, 0], [50, 50]]);
    }

    #[test]
    fn rate_follows_the_queue_depth_within_bounds() {
        let mut r = Resampler::new(SAMPLE_RATE, 48_000);
        for _ in 0..1000 {
            r.track(TARGET);
        }
        assert!((r.step - r.nominal).abs() < 1e-9, "on target: nominal rate");
        for _ in 0..1000 {
            r.track(HIGH_WATER);
        }
        let fast = r.step / r.nominal - 1.0;
        assert!(fast > 0.0 && fast <= MAX_RATE_SKEW + 1e-12, "full: {fast}");
        for _ in 0..1000 {
            r.track(0);
        }
        let slow = r.step / r.nominal - 1.0;
        assert!(
            (-MAX_RATE_SKEW - 1e-12..0.0).contains(&slow),
            "empty: {slow}"
        );
    }

    /// How often a simulated game run leaves the device without sound.
    /// The game emulates its frames in passes, one every `pass_ms`
    /// milliseconds, emulating up to `catch_up` frames if that many are
    /// due, as the game loop does; the device takes a 10 ms buffer at
    /// 48 kHz. Returns the number of dry spells after the first second.
    fn dry_spells(pass_ms: u32, catch_up: u32) -> usize {
        const FRAME_US: u32 = 16_743;
        let mut shared = Shared::primed();
        let mut r = Resampler::new(SAMPLE_RATE, 48_000);
        let per_frame = f64::from(SAMPLE_RATE) * f64::from(FRAME_US) / 1e6;
        let (mut made, mut next_frame_us, mut next_pass_us) = (0.0, 0u32, 0u32);
        let mut spells = 0;
        for ms in 0..20_000u32 {
            let now_us = ms * 1000;
            if now_us >= next_pass_us {
                let due = 1 + now_us.saturating_sub(next_frame_us) / FRAME_US;
                let frames = due.min(catch_up);
                made += per_frame * f64::from(frames);
                let whole = made as usize;
                shared.queue.extend(std::iter::repeat_n([1, 1], whole));
                made = made.fract();
                next_frame_us += FRAME_US * frames;
                next_pass_us = now_us + pass_ms * 1000;
                if now_us > next_frame_us + catch_up * FRAME_US {
                    next_frame_us = now_us;
                }
            }
            if ms % 10 == 0 {
                let was = shared.starved;
                shared.play(&mut r, 480, |_| {});
                if shared.starved && !was && ms > 1000 {
                    spells += 1;
                }
            }
        }
        spells
    }

    #[test]
    fn a_slow_terminal_does_not_starve_the_sound_once_frames_catch_up() {
        // A terminal taking 40 ms per drawn frame. One frame per pass —
        // how the loop used to run — makes 25 frames a second of sound
        // for a device playing 60: it runs dry several times a second.
        assert!(dry_spells(40, 1) > 50);
        // Catching up on the frames the draw cost keeps it fed, up to
        // draws as slow as the catch-up allows for.
        for pass_ms in [17, 25, 40, 60, 90] {
            assert_eq!(dry_spells(pass_ms, crate::MAX_CATCH_UP), 0, "{pass_ms} ms");
        }
    }

    #[test]
    fn rate_control_absorbs_clock_drift() {
        // The emulator makes 0.3 % more than the device plays at the
        // nominal rate. Without rate control the queue would grow past
        // the high water mark within a minute.
        let mut r = Resampler::new(SAMPLE_RATE, 48_000);
        let mut q: VecDeque<Frame> = std::iter::repeat_n([0, 0], TARGET).collect();
        let per_frame = f64::from(SAMPLE_RATE) / 59.73 * 1.003;
        let mut made = 0.0;
        for _ in 0..(60 * 60) {
            made += per_frame;
            while made >= 1.0 {
                q.push_back([0, 0]);
                made -= 1.0;
            }
            // One 10 ms device buffer and a bit, per 16.7 ms frame.
            for chunk in [480, 324] {
                r.track(q.len());
                for _ in 0..chunk {
                    r.next(&mut q).expect("never dry");
                }
            }
        }
        assert!(q.len() < HIGH_WATER, "settled at {}", q.len());
        assert!(q.len() > PRIME, "settled at {}", q.len());
    }
}
