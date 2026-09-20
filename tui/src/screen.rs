//! A Ratatui widget that draws the GBA framebuffer with half-block glyphs.
//!
//! Each terminal cell shows two vertically stacked pixels: the upper one
//! as the foreground colour of a `▀` glyph and the lower one as the cell
//! background. A 240×160 frame therefore needs 240×80 cells at 1:1; when
//! the terminal is smaller the image is downscaled by an integer factor,
//! averaging each block of source pixels.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

/// Upper half block; foreground = top pixel, background = bottom pixel.
const UPPER_HALF: char = '▀';

/// Cells needed to show the whole screen at 1:1.
pub const CELL_WIDTH: u16 = SCREEN_WIDTH as u16;
/// Rows needed to show the whole screen at 1:1.
pub const CELL_HEIGHT: u16 = (SCREEN_HEIGHT / 2) as u16;

/// Largest downscale factor worth trying before giving up and cropping.
const MAX_SCALE: u16 = 8;

/// Renders a borrowed [`Framebuffer`], centred in the area.
///
/// The scale factor is chosen automatically: 1:1 when it fits, otherwise
/// the smallest integer reduction that does. If even the maximum
/// reduction does not fit, the image is centre-cropped.
#[derive(Debug, Clone, Copy)]
pub struct GbaScreen<'a> {
    framebuffer: &'a Framebuffer,
}

impl<'a> GbaScreen<'a> {
    /// Wraps a framebuffer for rendering.
    #[must_use]
    pub const fn new(framebuffer: &'a Framebuffer) -> Self {
        Self { framebuffer }
    }

    /// The reduction factor used for `area` (1 = full resolution).
    #[must_use]
    pub fn scale_for(area: Rect) -> u16 {
        (1..=MAX_SCALE)
            .find(|&s| {
                CELL_WIDTH.div_ceil(s) <= area.width && CELL_HEIGHT.div_ceil(s) <= area.height
            })
            .unwrap_or(MAX_SCALE)
    }

    /// Whether `area` can show the full screen without cropping at the
    /// scale [`GbaScreen::scale_for`] would pick.
    #[must_use]
    pub fn fits(area: Rect) -> bool {
        let s = Self::scale_for(area);
        CELL_WIDTH.div_ceil(s) <= area.width && CELL_HEIGHT.div_ceil(s) <= area.height
    }

    /// Average colour of the `scale`×`scale` block whose top-left pixel is
    /// `(x, y)`, clipped to the screen.
    fn block_average(self, x: usize, y: usize, scale: usize) -> (u8, u8, u8) {
        let (mut sum, mut count) = ([0u32; 3], 0u32);
        for row in y..(y + scale).min(SCREEN_HEIGHT) {
            for &p in &self.framebuffer.row(row)[x..(x + scale).min(SCREEN_WIDTH)] {
                sum[0] += p >> 24;
                sum[1] += (p >> 16) & 0xFF;
                sum[2] += (p >> 8) & 0xFF;
                count += 1;
            }
        }
        if count == 0 {
            return (0, 0, 0);
        }
        (
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        )
    }
}

impl Widget for GbaScreen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let scale = Self::scale_for(area);
        let img_cols = CELL_WIDTH.div_ceil(scale);
        let img_rows = CELL_HEIGHT.div_ceil(scale);
        let cols = area.width.min(img_cols);
        let rows = area.height.min(img_rows);
        if cols == 0 || rows == 0 {
            return;
        }

        // Centre the visible window both on screen and within the image.
        let dst_x = area.x + (area.width - cols) / 2;
        let dst_y = area.y + (area.height - rows) / 2;
        let src_col = usize::from((img_cols - cols) / 2);
        let src_row = usize::from((img_rows - rows) / 2);
        let scale = usize::from(scale);

        for row in 0..usize::from(rows) {
            let y_top = (src_row + row) * 2 * scale;
            let y = dst_y + row as u16;
            for col in 0..usize::from(cols) {
                let x_px = (src_col + col) * scale;
                let x = dst_x + col as u16;
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };

                let (upper, lower) = if scale == 1 {
                    (
                        self.framebuffer.rgb(x_px, y_top),
                        self.framebuffer.rgb(x_px, y_top + 1),
                    )
                } else {
                    (
                        self.block_average(x_px, y_top, scale),
                        self.block_average(x_px, y_top + scale, scale),
                    )
                };

                cell.bg = Color::Rgb(lower.0, lower.1, lower.2);
                if upper == lower {
                    // Solid cell: a space with only a background keeps the
                    // terminal's escape output shorter.
                    cell.set_char(' ');
                    cell.fg = Color::Reset;
                } else {
                    cell.set_char(UPPER_HALF);
                    cell.fg = Color::Rgb(upper.0, upper.1, upper.2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame whose pixel at (x, y) encodes its coordinates in R and G.
    fn coordinate_frame() -> Framebuffer {
        let mut fb = Framebuffer::new();
        for y in 0..SCREEN_HEIGHT {
            for (x, px) in fb.row_mut(y).iter_mut().enumerate() {
                *px = ((x as u32) << 24) | ((y as u32) << 16) | 0xFF;
            }
        }
        fb
    }

    #[test]
    fn full_size_maps_two_pixels_per_cell() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, CELL_WIDTH, CELL_HEIGHT);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);

        let cell = &buf[(10, 3)];
        assert_eq!(cell.symbol(), "▀");
        assert_eq!(cell.fg, Color::Rgb(10, 6, 0));
        assert_eq!(cell.bg, Color::Rgb(10, 7, 0));

        let last = &buf[(CELL_WIDTH - 1, CELL_HEIGHT - 1)];
        assert_eq!(last.fg, Color::Rgb(239, 158, 0));
        assert_eq!(last.bg, Color::Rgb(239, 159, 0));
    }

    #[test]
    fn identical_pixels_collapse_to_background_only() {
        let mut fb = Framebuffer::new();
        fb.row_mut(0).fill(0x1122_33FF);
        fb.row_mut(1).fill(0x1122_33FF);
        let area = Rect::new(0, 0, CELL_WIDTH, CELL_HEIGHT);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        let cell = &buf[(0, 0)];
        assert_eq!(cell.symbol(), " ");
        assert_eq!(cell.bg, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(cell.fg, Color::Reset);
    }

    #[test]
    fn larger_area_centres_the_image() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, CELL_WIDTH + 20, CELL_HEIGHT + 10);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        assert_eq!(buf[(9, 5)].symbol(), " ", "left margin untouched");
        assert_eq!(buf[(9, 5)].bg, Color::Reset);
        assert_eq!(
            buf[(10, 5)].fg,
            Color::Rgb(0, 0, 0),
            "pixel (0,0) lands at cell (10,5)"
        );
        assert_eq!(buf[(10, 5)].bg, Color::Rgb(0, 1, 0));
    }

    #[test]
    fn picks_smallest_scale_that_fits() {
        assert_eq!(GbaScreen::scale_for(Rect::new(0, 0, 240, 80)), 1);
        assert_eq!(GbaScreen::scale_for(Rect::new(0, 0, 239, 80)), 2);
        assert_eq!(GbaScreen::scale_for(Rect::new(0, 0, 120, 40)), 2);
        assert_eq!(GbaScreen::scale_for(Rect::new(0, 0, 117, 27)), 3);
        assert_eq!(GbaScreen::scale_for(Rect::new(0, 0, 10, 5)), MAX_SCALE);
        assert!(GbaScreen::fits(Rect::new(0, 0, 117, 27)));
        assert!(!GbaScreen::fits(Rect::new(0, 0, 10, 5)));
    }

    #[test]
    fn downscale_averages_blocks() {
        let mut fb = Framebuffer::new();
        // 2x2 block at the top-left: two white, two black -> mid grey.
        fb.row_mut(0)[0] = 0xFFFF_FFFF;
        fb.row_mut(1)[1] = 0xFFFF_FFFF;
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        let cell = &buf[(0, 0)];
        assert_eq!(cell.fg, Color::Rgb(127, 127, 127), "upper block average");
        assert_eq!(cell.bg, Color::Rgb(0, 0, 0), "lower block is rows 2-3");
        assert_eq!(cell.symbol(), "▀");
        // Pixel (239, 159) maps to cell (119, 39).
        fb.row_mut(159)[239] = 0xFF00_00FF;
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        assert_eq!(buf[(119, 39)].bg, Color::Rgb(63, 0, 0));
    }

    #[test]
    fn odd_scale_covers_whole_image() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, 80, 27); // scale 3: 80 x 27 cells (last row partial)
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        assert_eq!(GbaScreen::scale_for(area), 3);
        // Last row covers pixel rows 156..159 (top 156-158, bottom 159).
        let cell = &buf[(0, 26)];
        assert_eq!(cell.fg, Color::Rgb(1, 157, 0));
        assert_eq!(cell.bg, Color::Rgb(1, 159, 0));
    }

    #[test]
    fn zero_area_is_a_no_op() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
    }
}
