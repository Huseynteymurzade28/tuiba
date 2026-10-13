//! Non-interactive runs for debugging: emulate a fixed number of frames
//! with scripted input, then report CPU state and optionally save a
//! screenshot.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::time::Instant;

use tuiba_core::apu::SAMPLE_RATE;
use tuiba_core::cpu::disasm;
use tuiba_core::memory::io::{KEYINPUT_ALL_RELEASED, reg};
use tuiba_core::{Gba, Memory, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::cli::{Disasm, Headless, KeyHold};
use crate::profile::Profile;
use crate::{clock, png, screenshot, trace};

/// One GBA frame, 280 896 cycles at 16.78 MHz, in microseconds.
const FRAME_MICROS: i64 = 16_743;

/// Where the cartridge clock starts when `--clock` is not given: a fixed
/// moment, so that two runs of a clock-reading game match.
pub const DEFAULT_CLOCK: &str = "2000-01-01T00:00:00";

/// Runs `gba` as configured and prints a one-line summary to stdout.
pub fn run(gba: &mut Gba, config: &Headless) -> io::Result<()> {
    let mut audio = Vec::new();
    let start = Instant::now();
    let clock_start = config
        .clock
        .or_else(|| clock::parse(DEFAULT_CLOCK))
        .expect("the default clock parses");
    let profile_from = config.profile.map(|n| config.frames.saturating_sub(n));
    let mut profile = Profile::default();
    if !config.trace_io.is_empty() {
        gba.trace_io(config.trace_io.iter().cloned());
    }
    for frame in 0..config.frames {
        let elapsed = chrono::TimeDelta::microseconds(i64::from(frame) * FRAME_MICROS);
        gba.set_clock(clock::from_chrono(clock_start + elapsed));
        gba.set_keyinput(keyinput_at(&config.keys, frame));
        if profile_from.is_some_and(|from| frame >= from) {
            profile.run_frame(gba);
        } else {
            gba.run_frame();
        }
        if config.wav.is_some() {
            audio.extend_from_slice(gba.audio());
        }
        if !config.trace_io.is_empty() {
            trace::write_log(io::stdout().lock(), frame, &gba.take_io_writes())?;
        }
        gba.clear_audio();
    }
    let elapsed = start.elapsed();

    if let Some(path) = &config.wav {
        let file = BufWriter::new(File::create(path)?);
        write_wav(file, &audio)?;
    }

    if let Some(path) = &config.screenshot {
        let rgb = screenshot::rgb(gba.framebuffer());
        let file = BufWriter::new(File::create(path)?);
        png::write_rgb(file, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32, &rgb)?;
    }

    let mut stdout = io::stdout().lock();
    for range in &config.disasm {
        write_disasm(&mut stdout, &gba.bus, *range)?;
    }
    if profile_from.is_some() {
        profile.write(&mut stdout, &gba.bus)?;
    }
    drop(stdout);

    let fps = f64::from(config.frames) / elapsed.as_secs_f64().max(f64::EPSILON);
    let swi = gba
        .last_unsupported_swi
        .map(|n| format!(" unsupported_swi={n:#04x}"))
        .unwrap_or_default();
    println!(
        "frames={} time={:.2}s ({fps:.0} fps) pc={:#010x} mode={:?}{} dispcnt={:#06x} frame={:016x}{swi}",
        config.frames,
        elapsed.as_secs_f64(),
        gba.cpu.next_pc(),
        gba.cpu.regs.mode(),
        if gba.cpu.halted { " halted" } else { "" },
        gba.bus.io.read16(reg::DISPCNT),
        frame_hash(gba.framebuffer().pixels()),
    );
    Ok(())
}

/// Writes one line per instruction in `range`: address, encoding, text.
fn write_disasm(mut out: impl Write, memory: &impl Memory, range: Disasm) -> io::Result<()> {
    let mut address = range.start & if range.thumb { !1 } else { !3 };
    while address < range.end {
        if range.thumb {
            let half = memory.read16(address);
            let next = memory.read16(address.wrapping_add(2));
            let (text, size) = disasm::thumb(address, half, next);
            let encoding = if size == 4 {
                format!("{half:04x} {next:04x}")
            } else {
                format!("{half:04x}")
            };
            writeln!(out, "{address:08x}  {encoding:<9}  {text}")?;
            address = address.wrapping_add(size);
        } else {
            let word = memory.read32(address);
            writeln!(
                out,
                "{address:08x}  {word:08x}   {}",
                disasm::arm(address, word)
            )?;
            address = address.wrapping_add(4);
        }
        if address == 0 {
            break;
        }
    }
    Ok(())
}

/// FNV-1a over the final frame's pixels: a fingerprint of what is on
/// screen, for comparing a run against a known-good one (the test-ROM
/// job in CI does) without keeping reference images around.
fn frame_hash(pixels: &[u32]) -> u64 {
    pixels
        .iter()
        .flat_map(|p| p.to_le_bytes())
        .fold(0xCBF2_9CE4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
        })
}

/// Writes interleaved stereo 16-bit samples as a canonical 44-byte-header
/// PCM WAV file.
fn write_wav(mut out: impl Write, samples: &[i16]) -> io::Result<()> {
    const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    let data_len = u32::try_from(samples.len() * 2)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "audio too long for WAV"))?;
    let block_align = CHANNELS * BITS / 8;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_len).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&CHANNELS.to_le_bytes())?;
    out.write_all(&SAMPLE_RATE.to_le_bytes())?;
    out.write_all(&(SAMPLE_RATE * u32::from(block_align)).to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&BITS.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()
}

/// The active-low `KEYINPUT` value for `frame` under the scripted holds.
fn keyinput_at(holds: &[KeyHold], frame: u32) -> u16 {
    holds
        .iter()
        .filter(|h| (h.from..h.to).contains(&frame))
        .fold(KEYINPUT_ALL_RELEASED, |bits, h| bits & !h.key.mask())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::GbaKey;

    #[test]
    fn disassembly_lists_each_instruction() {
        let mut rom = vec![0u8; 0x200];
        rom[0x100..0x108].copy_from_slice(&[
            0x01, 0x20, // movs r0, #1
            0x00, 0xF0, 0x00, 0xF8, // bl +4
            0x70, 0x47, // bx lr
        ]);
        let gba = Gba::new(tuiba_core::Cartridge::from_bytes(rom).unwrap());
        let mut out = Vec::new();
        let range = Disasm {
            thumb: true,
            start: 0x0800_0100,
            end: 0x0800_0108,
        };
        write_disasm(&mut out, &gba.bus, range).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "08000100  2001       movs r0, #1\n\
             08000102  f000 f800  bl 0x08000106\n\
             08000106  4770       bx lr\n"
        );
    }

    #[test]
    fn frame_hash_is_fnv1a() {
        // FNV-1a of no bytes is the offset basis.
        assert_eq!(frame_hash(&[]), 0xCBF2_9CE4_8422_2325);
        assert_ne!(frame_hash(&[0x0000_00FF]), frame_hash(&[0x0000_01FF]));
    }

    #[test]
    fn wav_header_describes_stereo_16_bit_pcm() {
        let mut out = Vec::new();
        write_wav(&mut out, &[1, -1, 2, -2]).unwrap();
        assert_eq!(out.len(), 44 + 8);
        assert_eq!(&out[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(u16::from_le_bytes(out[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(out[24..28].try_into().unwrap()), 32_768);
        assert_eq!(u32::from_le_bytes(out[40..44].try_into().unwrap()), 8);
        assert_eq!(&out[44..48], &[1, 0, 0xFF, 0xFF]);
    }

    #[test]
    fn scripted_keys_are_active_low_within_range() {
        let holds = [
            KeyHold {
                key: GbaKey::Start,
                from: 10,
                to: 12,
            },
            KeyHold {
                key: GbaKey::A,
                from: 11,
                to: 20,
            },
        ];
        assert_eq!(keyinput_at(&holds, 9), KEYINPUT_ALL_RELEASED);
        assert_eq!(
            keyinput_at(&holds, 10),
            KEYINPUT_ALL_RELEASED & !GbaKey::Start.mask()
        );
        assert_eq!(
            keyinput_at(&holds, 11),
            KEYINPUT_ALL_RELEASED & !(GbaKey::Start.mask() | GbaKey::A.mask())
        );
        assert_eq!(
            keyinput_at(&holds, 12),
            KEYINPUT_ALL_RELEASED & !GbaKey::A.mask()
        );
        assert_eq!(keyinput_at(&holds, 20), KEYINPUT_ALL_RELEASED);
    }
}
