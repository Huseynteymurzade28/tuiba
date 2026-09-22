//! Sound output through the default audio device.
//!
//! The emulator produces samples at its own rate (32 768 Hz, see
//! [`tuiba_core::apu`]) in bursts of one frame; the device asks for them
//! at its own rate in bursts of its own size. A queue sits between the
//! two, and the device callback resamples linearly out of it. Latency is
//! the queue depth: it starts at [`PRIME`] and is cut back to [`TARGET`]
//! whenever drift lets it grow past [`HIGH_WATER`].

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use tuiba_core::apu::SAMPLE_RATE;

/// A left/right pair at the emulator's rate.
type Frame = [i16; 2];

/// Silence queued before the first sample, so a late frame does not
/// starve the device straight away (~60 ms).
const PRIME: usize = 2048;
/// Depth the queue is trimmed back to (~100 ms).
const TARGET: usize = 3277;
/// Depth past which the queue is trimmed (~250 ms).
const HIGH_WATER: usize = 8192;

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
    queue: Arc<Mutex<VecDeque<Frame>>>,
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
        let queue = Arc::new(Mutex::new(VecDeque::from(vec![[0, 0]; PRIME])));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, &queue),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, &queue),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, &queue),
            _ => build::<f32>(&device, &config, &queue),
        }
        .map_err(|e| AudioError(e.to_string()))?;
        stream.play().map_err(|e| AudioError(e.to_string()))?;
        Ok(Self {
            queue,
            _stream: stream,
        })
    }

    /// Queues interleaved stereo samples at the emulator's rate.
    pub fn push(&self, samples: &[i16]) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.extend(samples.as_chunks::<2>().0.iter().copied());
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
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.clear();
        queue.extend(std::iter::repeat_n([0, 0], PRIME));
    }
}

/// Builds the output stream for sample type `T`, with the resampling
/// callback closed over its own state.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: &Arc<Mutex<VecDeque<Frame>>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<i16>,
{
    let channels = usize::from(config.channels).max(1);
    let mut resampler = Resampler::new(SAMPLE_RATE, config.sample_rate);
    let queue = Arc::clone(queue);
    device.build_output_stream(
        *config,
        move |out: &mut [T], _| {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            for frame in out.chunks_mut(channels) {
                let [left, right] = resampler.next(&mut queue);
                // Stereo goes to the first two channels; a mono device
                // gets the left, extra channels are silent.
                frame[0] = T::from_sample(left);
                if let Some(slot) = frame.get_mut(1) {
                    *slot = T::from_sample(right);
                }
                for slot in frame.iter_mut().skip(2) {
                    *slot = T::from_sample(0);
                }
            }
        },
        |_| {}, // Underruns and device hiccups are not worth interrupting a game for.
        None,
    )
}

/// Linear interpolation from the emulator's rate to the device's.
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Position between `prev` (0) and `next` (1).
    phase: f64,
    prev: Frame,
    next: Frame,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from) / f64::from(to.max(1)),
            phase: 0.0,
            prev: [0, 0],
            next: [0, 0],
        }
    }

    /// The next output frame. A starved queue fades to silence rather
    /// than holding the last sample as a DC level.
    fn next(&mut self, queue: &mut VecDeque<Frame>) -> Frame {
        self.phase += self.step;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.prev = self.next;
            self.next = queue.pop_front().unwrap_or([0, 0]);
        }
        let mix = |a: i16, b: i16| {
            let t = self.phase;
            (f64::from(a) * (1.0 - t) + f64::from(b) * t) as i16
        };
        [
            mix(self.prev[0], self.next[0]),
            mix(self.prev[1], self.next[1]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_interpolates_and_fades_when_starved() {
        // 2:1 downsample: every output frame consumes two inputs, and the
        // output trails the input by one frame (the interpolation window).
        let mut r = Resampler::new(2, 1);
        let mut q: VecDeque<Frame> = [[0, 0], [100, -100], [200, -200], [300, -300]].into();
        assert_eq!(r.next(&mut q), [0, 0]);
        assert_eq!(r.next(&mut q), [200, -200]);
        assert_eq!(r.next(&mut q), [0, 0], "starved: silence");

        // 1:2 upsample: a point halfway between neighbours.
        let mut r = Resampler::new(1, 2);
        let mut q: VecDeque<Frame> = [[0, 0], [100, 100]].into();
        let out: Vec<Frame> = (0..6).map(|_| r.next(&mut q)).collect();
        assert_eq!(out, [[0, 0], [0, 0], [0, 0], [0, 0], [50, 50], [100, 100]]);
    }
}
