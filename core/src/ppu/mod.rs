//! Picture Processing Unit: LCD timing and scanline rendering.
//!
//! # Timing
//!
//! ```text
//! one scanline = 240 visible dots + 68 HBlank dots = 308 dots = 1232 cycles
//! one frame    = 160 visible lines + 68 VBlank lines = 228 lines
//! ```
//!
//! The PPU is driven with the cycles the CPU just spent. It renders a
//! scanline into the [`Framebuffer`] the moment that line enters HBlank,
//! maintains `VCOUNT` and the `DISPSTAT` status bits, and raises the
//! VBlank / HBlank / VCount interrupts.

pub mod bitmap;
pub mod framebuffer;

pub use framebuffer::{Framebuffer, Rgba, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::memory::VideoMemory;
use crate::memory::io::{Interrupt, IoRegisters, reg};

/// Cycles per scanline.
pub const CYCLES_PER_LINE: u32 = 1232;
/// Cycle within a line at which HBlank begins.
pub const HBLANK_START: u32 = 960;
/// Total lines per frame, including VBlank.
pub const LINES_PER_FRAME: u16 = 228;
/// Cycles per frame.
pub const CYCLES_PER_FRAME: u32 = CYCLES_PER_LINE * LINES_PER_FRAME as u32;

/// `DISPSTAT` bit layout.
mod dispstat {
    pub const VBLANK: u16 = 1 << 0;
    pub const HBLANK: u16 = 1 << 1;
    pub const VCOUNT_MATCH: u16 = 1 << 2;
    pub const VBLANK_IRQ: u16 = 1 << 3;
    pub const HBLANK_IRQ: u16 = 1 << 4;
    pub const VCOUNT_IRQ: u16 = 1 << 5;
}

/// What happened during a [`Ppu::step`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Events {
    /// VBlank began: the frame is complete.
    pub vblank: bool,
    /// A visible line entered HBlank (never set during VBlank lines).
    pub hblank: bool,
}

/// The PPU state.
#[derive(Debug, Clone)]
pub struct Ppu {
    /// The last completed frame plus whatever lines of the current frame
    /// have been rendered so far.
    pub framebuffer: Framebuffer,
    /// Current scanline (`VCOUNT`).
    vcount: u16,
    /// Cycle within the current scanline.
    line_cycle: u32,
    /// Whether HBlank has been entered on the current line.
    in_hblank: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    /// A PPU at the start of line 0.
    #[must_use]
    pub fn new() -> Self {
        Self {
            framebuffer: Framebuffer::new(),
            vcount: 0,
            line_cycle: 0,
            in_hblank: false,
        }
    }

    /// Current scanline.
    #[must_use]
    pub fn vcount(&self) -> u16 {
        self.vcount
    }

    /// Advances the PPU by `cycles` and reports the events that occurred.
    pub fn step(&mut self, cycles: u32, io: &mut IoRegisters, video: &VideoMemory) -> Events {
        self.line_cycle += cycles;
        let mut events = Events::default();

        loop {
            if !self.in_hblank && self.line_cycle >= HBLANK_START {
                events.hblank |= self.enter_hblank(io, video);
            }
            if self.line_cycle < CYCLES_PER_LINE {
                break;
            }
            self.line_cycle -= CYCLES_PER_LINE;
            events.vblank |= self.end_line(io);
        }
        events
    }

    /// Returns `true` if this was a visible line (HBlank DMA fires).
    fn enter_hblank(&mut self, io: &mut IoRegisters, video: &VideoMemory) -> bool {
        self.in_hblank = true;
        let visible = usize::from(self.vcount) < SCREEN_HEIGHT;
        if visible {
            self.render_line(io, video);
        }
        let stat = io.read16(reg::DISPSTAT);
        io.set_raw16(reg::DISPSTAT, stat | dispstat::HBLANK);
        // The HBlank interrupt fires on every line, VBlank lines included.
        if stat & dispstat::HBLANK_IRQ != 0 {
            io.request_interrupt(Interrupt::HBlank);
        }
        visible
    }

    /// Moves to the next line; returns `true` when entering VBlank.
    fn end_line(&mut self, io: &mut IoRegisters) -> bool {
        self.in_hblank = false;
        self.vcount = (self.vcount + 1) % LINES_PER_FRAME;
        io.set_raw16(reg::VCOUNT, self.vcount);

        let mut stat = io.read16(reg::DISPSTAT) & !(dispstat::HBLANK | dispstat::VCOUNT_MATCH);
        let mut entered_vblank = false;

        match self.vcount {
            160 => {
                stat |= dispstat::VBLANK;
                entered_vblank = true;
                if stat & dispstat::VBLANK_IRQ != 0 {
                    io.request_interrupt(Interrupt::VBlank);
                }
            }
            // The flag drops one line early, on line 227.
            227 => stat &= !dispstat::VBLANK,
            _ => {}
        }

        if self.vcount == stat >> 8 {
            stat |= dispstat::VCOUNT_MATCH;
            if stat & dispstat::VCOUNT_IRQ != 0 {
                io.request_interrupt(Interrupt::VCount);
            }
        }

        io.set_raw16(reg::DISPSTAT, stat);
        entered_vblank
    }

    fn render_line(&mut self, io: &IoRegisters, video: &VideoMemory) {
        let dispcnt = io.read16(reg::DISPCNT);
        let y = usize::from(self.vcount);
        let out = self.framebuffer.row_mut(y);

        // Forced blank: the LCD shows white.
        if dispcnt & (1 << 7) != 0 {
            out.fill(0xFFFF_FFFF);
            return;
        }

        let frame1 = dispcnt & (1 << 4) != 0;
        let bg2_enabled = dispcnt & (1 << 10) != 0;
        match dispcnt & 0x7 {
            3 if bg2_enabled => bitmap::render_mode3(video, y, out),
            4 if bg2_enabled => bitmap::render_mode4(video, frame1, y, out),
            5 if bg2_enabled => bitmap::render_mode5(video, frame1, y, out),
            // Tiled modes are not implemented yet; show the backdrop.
            _ => out.fill(bitmap::palette_color(video, 0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Ppu, IoRegisters, VideoMemory) {
        (Ppu::new(), IoRegisters::new(), VideoMemory::new())
    }

    #[test]
    fn hblank_and_line_advance() {
        let (mut ppu, mut io, video) = setup();
        assert_eq!(
            ppu.step(HBLANK_START - 1, &mut io, &video),
            Events::default()
        );
        assert_eq!(io.read16(reg::DISPSTAT) & dispstat::HBLANK, 0);
        assert!(ppu.step(1, &mut io, &video).hblank);
        assert_ne!(io.read16(reg::DISPSTAT) & dispstat::HBLANK, 0);
        assert_eq!(ppu.vcount(), 0);
        ppu.step(CYCLES_PER_LINE - HBLANK_START, &mut io, &video);
        assert_eq!(ppu.vcount(), 1);
        assert_eq!(io.read16(reg::VCOUNT), 1);
        assert_eq!(io.read16(reg::DISPSTAT) & dispstat::HBLANK, 0);
    }

    #[test]
    fn vblank_spans_lines_160_to_226() {
        let (mut ppu, mut io, video) = setup();
        let mut frames = 0;
        for line in 0..LINES_PER_FRAME {
            let events = ppu.step(CYCLES_PER_LINE, &mut io, &video);
            frames += u32::from(events.vblank);
            let next = (line + 1) % LINES_PER_FRAME;
            assert_eq!(events.vblank, next == 160, "line {next}");
            assert_eq!(events.hblank, line < 160, "hblank event on line {line}");
            let vblank = io.read16(reg::DISPSTAT) & dispstat::VBLANK != 0;
            assert_eq!(
                vblank,
                (160..227).contains(&next),
                "vblank flag on line {next}"
            );
        }
        assert_eq!(frames, 1);
        assert_eq!(ppu.vcount(), 0);
    }

    #[test]
    fn interrupts_follow_dispstat_enables() {
        let (mut ppu, mut io, video) = setup();
        io.write16(
            reg::DISPSTAT,
            dispstat::VBLANK_IRQ | dispstat::HBLANK_IRQ | dispstat::VCOUNT_IRQ | (5 << 8),
        );
        ppu.step(HBLANK_START, &mut io, &video);
        assert_eq!(io.read16(reg::IF), Interrupt::HBlank.mask());
        io.write16(reg::IF, 0xFFFF);
        ppu.step(CYCLES_PER_LINE * 5, &mut io, &video);
        assert_eq!(ppu.vcount(), 5);
        assert_ne!(io.read16(reg::IF) & Interrupt::VCount.mask(), 0);
        assert_ne!(io.read16(reg::DISPSTAT) & dispstat::VCOUNT_MATCH, 0);
        io.write16(reg::IF, 0xFFFF);
        ppu.step(CYCLES_PER_LINE * 155, &mut io, &video);
        assert_eq!(ppu.vcount(), 160);
        assert_ne!(io.read16(reg::IF) & Interrupt::VBlank.mask(), 0);
    }

    #[test]
    fn large_steps_catch_up_multiple_lines() {
        let (mut ppu, mut io, video) = setup();
        assert!(ppu.step(CYCLES_PER_FRAME, &mut io, &video).vblank);
        assert_eq!(ppu.vcount(), 0);
    }

    #[test]
    fn renders_mode3_line_at_hblank() {
        let (mut ppu, mut io, mut video) = setup();
        io.write16(reg::DISPCNT, 0x0403); // mode 3, BG2 on
        video.vram[0..2].copy_from_slice(&0x001Fu16.to_le_bytes());
        ppu.step(HBLANK_START - 1, &mut io, &video);
        assert_eq!(ppu.framebuffer.row(0)[0], 0x0000_00FF, "not rendered yet");
        ppu.step(1, &mut io, &video);
        assert_eq!(ppu.framebuffer.row(0)[0], 0xFF00_00FF);
    }

    #[test]
    fn forced_blank_is_white_and_disabled_bg_shows_backdrop() {
        let (mut ppu, mut io, mut video) = setup();
        video.palette[0..2].copy_from_slice(&0x03E0u16.to_le_bytes());
        io.write16(reg::DISPCNT, 0x0083);
        ppu.step(HBLANK_START, &mut io, &video);
        assert_eq!(ppu.framebuffer.row(0)[100], 0xFFFF_FFFF);
        io.write16(reg::DISPCNT, 0x0003); // mode 3 but BG2 off
        ppu.step(CYCLES_PER_LINE, &mut io, &video);
        assert_eq!(ppu.framebuffer.row(1)[100], 0x00FF_00FF);
    }
}
