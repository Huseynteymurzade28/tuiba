//! Scanline composition: windows, priority ordering and colour effects.

use crate::memory::io::{IoRegisters, reg};
use crate::ppu::framebuffer::SCREEN_WIDTH;
use crate::ppu::obj::ObjLine;
use crate::ppu::tiled::TRANSPARENT;

/// Layer identifiers as used in `WININ`/`WINOUT`/`BLDCNT` bit order.
const OBJ: usize = 4;
const BACKDROP: usize = 5;

/// Which layers (and colour effects) are visible at a pixel.
#[derive(Debug, Clone, Copy)]
struct WindowMask {
    layers: u8,
    effects: bool,
}

impl WindowMask {
    const ALL: Self = Self {
        layers: 0x3F,
        effects: true,
    };

    fn from_bits(bits: u16) -> Self {
        Self {
            layers: (bits & 0x1F) as u8,
            effects: bits & 0x20 != 0,
        }
    }

    fn allows(self, layer: usize) -> bool {
        self.layers & (1 << layer) != 0
    }
}

/// A horizontal window range (`x1..x2`) on this line, if the window is
/// active on this line at all.
fn window_range(io: &IoRegisters, h: u32, v: u32, y: usize) -> Option<(usize, usize)> {
    let (hv, vv) = (io.read16(h), io.read16(v));
    let (y1, y2) = (usize::from(vv >> 8), usize::from(vv & 0xFF));
    // Ranges are inclusive-exclusive; an inverted range wraps to the edge.
    let y2 = if y2 < y1 || y2 > 160 { 160 } else { y2 };
    if y < y1 || y >= y2 {
        return None;
    }
    let (x1, x2) = (usize::from(hv >> 8), usize::from(hv & 0xFF));
    let x2 = if x2 < x1 || x2 > SCREEN_WIDTH {
        SCREEN_WIDTH
    } else {
        x2
    };
    Some((x1, x2))
}

/// Decoded `BLDCNT`/`BLDALPHA`/`BLDY`.
#[derive(Debug, Clone, Copy)]
struct Blend {
    first: u8,
    second: u8,
    effect: u8,
    eva: u32,
    evb: u32,
    evy: u32,
}

impl Blend {
    fn read(io: &IoRegisters) -> Self {
        let cnt = io.read16(reg::BLDCNT);
        let alpha = io.read16(reg::BLDALPHA);
        Self {
            first: (cnt & 0x3F) as u8,
            second: ((cnt >> 8) & 0x3F) as u8,
            effect: ((cnt >> 6) & 0b11) as u8,
            eva: u32::from(alpha & 0x1F).min(16),
            evb: u32::from((alpha >> 8) & 0x1F).min(16),
            evy: u32::from(io.read16(reg::BLDY) & 0x1F).min(16),
        }
    }
}

/// Per-channel `a*wa + b*wb` on 15-bit colours, saturating at 31.
fn mix(a: u16, wa: u32, b: u16, wb: u32) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        let ca = u32::from(a >> shift) & 0x1F;
        let cb = u32::from(b >> shift) & 0x1F;
        let c = ((ca * wa + cb * wb) >> 4).min(31);
        out |= (c as u16) << shift;
    }
    out
}

fn brighten(c: u16, evy: u32) -> u16 {
    mix(c, 16 - evy, 0x7FFF, evy)
}

fn darken(c: u16, evy: u32) -> u16 {
    mix(c, 16 - evy, 0, 0)
}

/// Composes one scanline into 15-bit colours.
///
/// `bg_lines` holds each background's pixels (`TRANSPARENT` where empty),
/// `bg_enabled`/`bg_priority` its state, `obj` the sprite layer, and
/// `backdrop` palette entry 0.
#[allow(clippy::too_many_arguments)]
pub fn compose_line(
    io: &IoRegisters,
    y: usize,
    bg_lines: &[[u16; SCREEN_WIDTH]; 4],
    bg_enabled: [bool; 4],
    bg_priority: [u8; 4],
    obj_enabled: bool,
    obj: &ObjLine,
    backdrop: u16,
    out: &mut [u16; SCREEN_WIDTH],
) {
    let dispcnt = io.read16(reg::DISPCNT);
    let masks = window_masks(io, dispcnt, y, obj_enabled, obj);
    let blend = Blend::read(io);

    // Visible backgrounds front to back: by priority, ties to the
    // lower-numbered layer.
    let mut order = [(0u8, 0usize); 4];
    let mut count = 0;
    for bg in 0..4 {
        if bg_enabled[bg] {
            order[count] = (bg_priority[bg], bg);
            count += 1;
        }
    }
    let order = &mut order[..count];
    order.sort_unstable();

    for (x, px) in out.iter_mut().enumerate() {
        let mask = masks[x];
        let (top, second, semi) =
            select_layers(x, mask, bg_lines, order, obj_enabled, obj, backdrop);

        let (color, layer) = top;
        let first_target = blend.first & (1 << layer) != 0;
        let second_target = blend.second & (1 << second.1) != 0;

        *px = if !mask.effects {
            color
        } else if semi && second_target {
            mix(color, blend.eva, second.0, blend.evb)
        } else if !first_target {
            color
        } else {
            match blend.effect {
                1 if second_target => mix(color, blend.eva, second.0, blend.evb),
                2 => brighten(color, blend.evy),
                3 => darken(color, blend.evy),
                _ => color,
            }
        };
    }
}

/// Resolves the windows of line `y` once, so the pixel loop only looks
/// a mask up. WIN0 beats WIN1 beats the OBJ window beats outside.
fn window_masks(
    io: &IoRegisters,
    dispcnt: u16,
    y: usize,
    obj_enabled: bool,
    obj: &ObjLine,
) -> [WindowMask; SCREEN_WIDTH] {
    let win0_on = dispcnt & (1 << 13) != 0;
    let win1_on = dispcnt & (1 << 14) != 0;
    let objwin_on = dispcnt & (1 << 15) != 0 && obj_enabled;
    if !(win0_on || win1_on || objwin_on) {
        return [WindowMask::ALL; SCREEN_WIDTH];
    }

    let winin = io.read16(reg::WININ);
    let winout = io.read16(reg::WINOUT);
    let mut masks = [WindowMask::from_bits(winout); SCREEN_WIDTH];
    if objwin_on {
        let mask_obj = WindowMask::from_bits(winout >> 8);
        for (mask, &inside) in masks.iter_mut().zip(&obj.window) {
            if inside {
                *mask = mask_obj;
            }
        }
    }
    let ranges = [
        (win1_on, reg::WIN1H, reg::WIN1V, winin >> 8),
        (win0_on, reg::WIN0H, reg::WIN0V, winin),
    ];
    for (on, h, v, bits) in ranges {
        if let Some((x1, x2)) = on.then(|| window_range(io, h, v, y)).flatten() {
            // x1 can lie past the screen edge, where x2 is clamped to it.
            masks[x1.min(x2)..x2].fill(WindowMask::from_bits(bits));
        }
    }
    masks
}

/// Picks the two topmost visible pixels at `x` as `(colour, layer id)`,
/// plus whether the top one is a semi-transparent sprite. `order` lists
/// the enabled backgrounds front to back as `(priority, layer)`.
fn select_layers(
    x: usize,
    mask: WindowMask,
    bg_lines: &[[u16; SCREEN_WIDTH]; 4],
    order: &[(u8, usize)],
    obj_enabled: bool,
    obj: &ObjLine,
    backdrop: u16,
) -> ((u16, usize), (u16, usize), bool) {
    let mut top = (backdrop, BACKDROP);
    let mut second = (backdrop, BACKDROP);
    let mut top_priority = 4u8;
    let mut second_priority = 4u8;

    let mut found = false;
    for &(priority, bg) in order {
        if !mask.allows(bg) {
            continue;
        }
        let c = bg_lines[bg][x];
        if c == TRANSPARENT {
            continue;
        }
        if found {
            second = (c, bg);
            second_priority = priority;
            break;
        }
        top = (c, bg);
        top_priority = priority;
        found = true;
    }

    let mut semi = false;
    if obj_enabled && mask.allows(OBJ) && obj.colors[x] != TRANSPARENT {
        if obj.priorities[x] <= top_priority {
            second = top;
            top = (obj.colors[x], OBJ);
            semi = obj.semi_transparent[x];
        } else if second.1 == BACKDROP || obj.priorities[x] <= second_priority {
            second = (obj.colors[x], OBJ);
        }
    }
    (top, second, semi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> [[u16; SCREEN_WIDTH]; 4] {
        let mut l = [[TRANSPARENT; SCREEN_WIDTH]; 4];
        l[0].fill(0x001F); // BG0 red everywhere
        l[1].fill(0x03E0); // BG1 green everywhere
        l
    }

    fn compose(io: &IoRegisters, obj: &ObjLine, prio: [u8; 4]) -> [u16; SCREEN_WIDTH] {
        let mut out = [0; SCREEN_WIDTH];
        compose_line(
            io,
            10,
            &lines(),
            [true, true, false, false],
            prio,
            true,
            obj,
            0x7C00,
            &mut out,
        );
        out
    }

    #[test]
    fn priority_and_backdrop() {
        let io = IoRegisters::new();
        let obj = ObjLine::default();
        assert_eq!(
            compose(&io, &obj, [1, 0, 0, 0])[0],
            0x03E0,
            "BG1 wins with better priority"
        );
        assert_eq!(
            compose(&io, &obj, [0, 0, 0, 0])[0],
            0x001F,
            "tie goes to BG0"
        );
        let mut out = [0; SCREEN_WIDTH];
        compose_line(
            &io,
            0,
            &lines(),
            [false; 4],
            [0; 4],
            false,
            &obj,
            0x7C00,
            &mut out,
        );
        assert_eq!(out[5], 0x7C00, "backdrop");
    }

    #[test]
    fn sprites_respect_priority_against_backgrounds() {
        let io = IoRegisters::new();
        let mut obj = ObjLine::default();
        obj.colors[0] = 0x7FFF;
        obj.priorities[0] = 1;
        assert_eq!(
            compose(&io, &obj, [1, 1, 0, 0])[0],
            0x7FFF,
            "equal priority: sprite on top"
        );
        assert_eq!(
            compose(&io, &obj, [0, 1, 0, 0])[0],
            0x001F,
            "BG0 priority 0 covers it"
        );
    }

    #[test]
    fn windows_mask_layers() {
        let mut io = IoRegisters::new();
        io.write16(reg::DISPCNT, 1 << 13); // WIN0 on
        io.write16(reg::WIN0H, 0x0A14); // x 10..20
        io.write16(reg::WIN0V, 0x00A0); // y 0..160
        io.write16(reg::WININ, 0b0000_0010); // inside: only BG1
        io.write16(reg::WINOUT, 0b0000_0001); // outside: only BG0
        let out = compose(&io, &ObjLine::default(), [0; 4]);
        assert_eq!(out[15], 0x03E0);
        assert_eq!(out[5], 0x001F);
        assert_eq!(out[20], 0x001F, "x2 is exclusive");
        // Outside the vertical range the window does not apply.
        io.write16(reg::WIN0V, 0x141E); // y 20..30
        assert_eq!(compose(&io, &ObjLine::default(), [0; 4])[15], 0x001F);
    }

    #[test]
    fn window_starting_past_the_edge_covers_nothing() {
        let mut io = IoRegisters::new();
        io.write16(reg::DISPCNT, 1 << 13);
        io.write16(reg::WIN0H, 0xFA00); // x1 = 250, x2 wraps to the edge
        io.write16(reg::WIN0V, 0x00A0);
        io.write16(reg::WININ, 0b0000_0010);
        io.write16(reg::WINOUT, 0b0000_0001);
        let out = compose(&io, &ObjLine::default(), [0; 4]);
        assert!(out.iter().all(|&c| c == 0x001F));
    }

    #[test]
    fn second_target_is_the_next_layer_by_priority() {
        let mut l = [[TRANSPARENT; SCREEN_WIDTH]; 4];
        l[0].fill(0x001F); // red, priority 1
        l[1].fill(0x03E0); // green, priority 1
        l[2].fill(0x7C00); // blue, priority 0: on top
        let mut io = IoRegisters::new();
        // BG2 first target; BG0 and BG1 second targets; alpha 16/16.
        io.write16(reg::BLDCNT, 0b100 | (1 << 6) | (0b011 << 8));
        io.write16(reg::BLDALPHA, 16 | (16 << 8));
        let mut out = [0; SCREEN_WIDTH];
        compose_line(
            &io,
            0,
            &l,
            [true, true, true, false],
            [1, 1, 0, 0],
            false,
            &ObjLine::default(),
            0,
            &mut out,
        );
        assert_eq!(out[0], 0x7C00 | 0x001F, "blue + red: BG0 wins the tie");
    }

    #[test]
    fn obj_window_uses_winout_high_byte() {
        let mut io = IoRegisters::new();
        io.write16(reg::DISPCNT, 1 << 15);
        io.write16(reg::WINOUT, 0b0000_0001 | (0b0000_0010 << 8));
        let mut obj = ObjLine::default();
        obj.window[7] = true;
        let out = compose(&io, &obj, [0; 4]);
        assert_eq!(out[7], 0x03E0, "inside obj window: BG1");
        assert_eq!(out[8], 0x001F, "outside: BG0");
    }

    #[test]
    fn alpha_blending_and_brightness() {
        let mut io = IoRegisters::new();
        // BG0 (first) over BG1 (second), alpha 8/16 each.
        io.write16(reg::BLDCNT, 0b01 | (1 << 6) | (0b10 << 8));
        io.write16(reg::BLDALPHA, 8 | (8 << 8));
        let out = compose(&io, &ObjLine::default(), [0; 4]);
        assert_eq!(out[0], 0x000F | (0x0F << 5), "half red + half green");

        io.write16(reg::BLDCNT, 0b01 | (2 << 6)); // brighten BG0
        io.write16(reg::BLDY, 16);
        assert_eq!(compose(&io, &ObjLine::default(), [0; 4])[0], 0x7FFF);
        io.write16(reg::BLDCNT, 0b01 | (3 << 6)); // darken BG0
        assert_eq!(compose(&io, &ObjLine::default(), [0; 4])[0], 0);
        io.write16(reg::BLDCNT, 0b10 | (3 << 6)); // darken only BG1: BG0 untouched
        assert_eq!(compose(&io, &ObjLine::default(), [0; 4])[0], 0x001F);
    }

    #[test]
    fn semi_transparent_sprite_blends_over_second_target() {
        let mut io = IoRegisters::new();
        io.write16(reg::BLDCNT, 0b01 << 8); // BG0 is a second target; no effect selected
        io.write16(reg::BLDALPHA, 8 | (8 << 8));
        let mut obj = ObjLine::default();
        obj.colors[0] = 0x7FFF;
        obj.semi_transparent[0] = true;
        let out = compose(&io, &obj, [0; 4]);
        assert_eq!(
            out[0],
            0x1F | (0x0F << 5) | (0x0F << 10),
            "white over red at 50%"
        );
        // Effects disabled by window: plain sprite colour.
        io.write16(reg::DISPCNT, 1 << 13);
        io.write16(reg::WIN0H, 240);
        io.write16(reg::WIN0V, 160);
        io.write16(reg::WININ, 0x1F); // all layers, no effects
        assert_eq!(compose(&io, &obj, [0; 4])[0], 0x7FFF);
    }
}
