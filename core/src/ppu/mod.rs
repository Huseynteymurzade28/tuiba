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
pub mod compose;
pub mod framebuffer;
pub mod obj;
pub mod tiled;

pub use framebuffer::{Framebuffer, Rgba, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::memory::VideoMemory;
use crate::memory::io::{Interrupt, IoRegisters, reg};
use framebuffer::bgr555_to_rgba;
use tiled::{BgControl, Mosaic, TRANSPARENT};

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
    /// Per-background scanline scratch buffers, 15-bit colours.
    bg_lines: [[u16; SCREEN_WIDTH]; 4],
    /// OBJ layer scratch.
    obj_line: obj::ObjLine,
    /// Composed 15-bit scanline before conversion to RGBA.
    composed: [u16; SCREEN_WIDTH],
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
            bg_lines: [[TRANSPARENT; SCREEN_WIDTH]; 4],
            obj_line: obj::ObjLine::default(),
            composed: [0; SCREEN_WIDTH],
        }
    }

    /// Current scanline.
    #[must_use]
    pub fn vcount(&self) -> u16 {
        self.vcount
    }

    /// Jumps to the start of scanline `line` and mirrors it into `VCOUNT`.
    ///
    /// Used to reproduce where the BIOS hands control to the cartridge.
    pub fn set_line(&mut self, line: u16, io: &mut IoRegisters) {
        self.vcount = line % LINES_PER_FRAME;
        self.line_cycle = 0;
        self.in_hblank = false;
        io.set_raw16(reg::VCOUNT, self.vcount);
        let mut stat = io.read16(reg::DISPSTAT) & !(dispstat::VBLANK | dispstat::HBLANK);
        if (160..227).contains(&self.vcount) {
            stat |= dispstat::VBLANK;
        }
        io.set_raw16(reg::DISPSTAT, stat);
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

    /// Renders the current scanline: each enabled background into its
    /// scratch buffer, then composited front-to-back by priority.
    fn render_line(&mut self, io: &IoRegisters, video: &VideoMemory) {
        let dispcnt = io.read16(reg::DISPCNT);
        let y = usize::from(self.vcount);

        // Forced blank: the LCD shows white.
        if dispcnt & (1 << 7) != 0 {
            self.framebuffer.row_mut(y).fill(0xFFFF_FFFF);
            return;
        }

        let mode = dispcnt & 0x7;
        let frame1 = dispcnt & (1 << 4) != 0;
        let mut enabled = [false; 4];
        for (bg, on) in enabled.iter_mut().enumerate() {
            *on = dispcnt & (1 << (8 + bg)) != 0;
        }
        // Which backgrounds exist in this mode, and which are affine.
        let (available, affine): ([bool; 4], [bool; 4]) = match mode {
            0 => ([true; 4], [false; 4]),
            1 => ([true, true, true, false], [false, false, true, false]),
            2 => ([false, false, true, true], [false, false, true, true]),
            3..=5 => ([false, false, true, false], [false; 4]),
            _ => ([false; 4], [false; 4]),
        };

        let mosaic = Mosaic::read(io);
        for bg in 0..4 {
            enabled[bg] &= available[bg];
            if !enabled[bg] {
                continue;
            }
            let control = BgControl::read(io, bg);
            // Mosaic backgrounds repeat the first line of each block: draw
            // that line instead, then spread pixels across the block.
            let src_y = if control.mosaic {
                tiled::mosaic_snap(y, mosaic.bg_v)
            } else {
                y
            };
            let line = &mut self.bg_lines[bg];
            match mode {
                3 => bitmap::render_mode3(video, src_y, line),
                4 => bitmap::render_mode4(video, frame1, src_y, line),
                5 => bitmap::render_mode5(video, frame1, src_y, line),
                _ if affine[bg] => tiled::render_affine(io, video, bg, src_y, line),
                _ => tiled::render_text(io, video, bg, src_y, line),
            }
            if control.mosaic {
                tiled::mosaic_h(line, mosaic.bg_h);
            }
        }

        let objects = dispcnt & (1 << 12) != 0;
        if objects {
            obj::render_line(io, video, y, &mut self.obj_line);
        }

        let priorities: [u8; 4] = std::array::from_fn(|bg| BgControl::read(io, bg).priority);
        let backdrop = bitmap::palette_entry(video, 0);
        compose::compose_line(
            io,
            y,
            &self.bg_lines,
            enabled,
            priorities,
            objects,
            &self.obj_line,
            backdrop,
            &mut self.composed,
        );
        let out = self.framebuffer.row_mut(y);
        for (px, &c) in out.iter_mut().zip(self.composed.iter()) {
            *px = bgr555_to_rgba(c);
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
    fn set_line_updates_vcount_and_vblank_flag() {
        let (mut ppu, mut io, video) = setup();
        ppu.set_line(126, &mut io);
        assert_eq!(io.read16(reg::VCOUNT), 126);
        assert_eq!(io.read16(reg::DISPSTAT) & dispstat::VBLANK, 0);
        ppu.set_line(200, &mut io);
        assert_ne!(io.read16(reg::DISPSTAT) & dispstat::VBLANK, 0);
        ppu.set_line(126, &mut io);
        // 34 lines to VBlank from here.
        let events = ppu.step(CYCLES_PER_LINE * 34, &mut io, &video);
        assert!(events.vblank);
        assert_eq!(ppu.vcount(), 160);
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
    fn background_mosaic_snaps_to_blocks() {
        let (mut ppu, mut io, mut video) = setup();
        // Mode 3 with BG2 mosaic; 4×3 blocks.
        io.write16(reg::DISPCNT, 0x0403);
        io.write16(reg::BG2CNT, 1 << 6);
        io.write16(reg::MOSAIC, 0x23);
        // Pixel (x, y) = colour x + 32 * y, so each is unique.
        for y in 0..8 {
            for x in 0..16 {
                let c = (x + 32 * y) as u16;
                let o = (y * SCREEN_WIDTH + x) * 2;
                video.vram[o..o + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        ppu.step(CYCLES_PER_LINE * 5, &mut io, &video);
        let px = |x: usize, y: usize| ppu.framebuffer.row(y)[x] >> 8 & 0xFF;
        let expect = |x: usize, y: usize| bgr555_to_rgba((x + 32 * y) as u16) >> 8 & 0xFF;
        assert_eq!(px(0, 0), expect(0, 0));
        assert_eq!(px(3, 0), expect(0, 0), "x snaps to the block start");
        assert_eq!(px(4, 0), expect(4, 0));
        assert_eq!(px(5, 2), expect(4, 0), "y snaps to the block top");
        assert_eq!(px(9, 4), expect(8, 3));
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
