//! Non-interactive runs for debugging: emulate a fixed number of frames
//! with scripted input, then report CPU state and optionally save a
//! screenshot.

use std::fs::File;
use std::io::{self, BufWriter};
use std::time::Instant;

use tuiba_core::memory::io::{KEYINPUT_ALL_RELEASED, reg};
use tuiba_core::{Gba, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::cli::{Headless, KeyHold};
use crate::png;

/// Runs `gba` as configured and prints a one-line summary to stdout.
pub fn run(gba: &mut Gba, config: &Headless) -> io::Result<()> {
    let start = Instant::now();
    for frame in 0..config.frames {
        gba.set_keyinput(keyinput_at(&config.keys, frame));
        gba.run_frame();
    }
    let elapsed = start.elapsed();

    if let Some(path) = &config.screenshot {
        let fb = gba.framebuffer();
        let rgb: Vec<u8> = fb
            .pixels()
            .iter()
            .flat_map(|&p| [(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8])
            .collect();
        let file = BufWriter::new(File::create(path)?);
        png::write_rgb(file, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32, &rgb)?;
    }

    let fps = f64::from(config.frames) / elapsed.as_secs_f64().max(f64::EPSILON);
    let swi = gba
        .last_unsupported_swi
        .map(|n| format!(" unsupported_swi={n:#04x}"))
        .unwrap_or_default();
    println!(
        "frames={} time={:.2}s ({fps:.0} fps) pc={:#010x} mode={:?}{} dispcnt={:#06x}{swi}",
        config.frames,
        elapsed.as_secs_f64(),
        gba.cpu.next_pc(),
        gba.cpu.regs.mode(),
        if gba.cpu.halted { " halted" } else { "" },
        gba.bus.io.read16(reg::DISPCNT),
    );
    Ok(())
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
