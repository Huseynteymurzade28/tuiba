//! A Ratatui widget that draws the GBA framebuffer with half-block glyphs.
//!
//! Each terminal cell shows two vertically stacked pixels: the upper one
//! as the foreground colour of a `▀` glyph and the lower one as the cell
//! background. A 240×160 frame therefore needs exactly 240×80 cells.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

/// Upper half block; foreground = top pixel, background = bottom pixel.
const UPPER_HALF: char = '▀';

/// Cells needed to show the whole screen.
pub const CELL_WIDTH: u16 = SCREEN_WIDTH as u16;
/// Rows needed to show the whole screen.
pub const CELL_HEIGHT: u16 = (SCREEN_HEIGHT / 2) as u16;

/// Renders a borrowed [`Framebuffer`] at 1:1 pixel scale, centred in the
/// area. If the area is smaller than 240×80 the image is centre-cropped.
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

    /// Whether `area` can show the full screen without cropping.
    #[must_use]
    pub const fn fits(area: Rect) -> bool {
        area.width >= CELL_WIDTH && area.height >= CELL_HEIGHT
    }
}

impl Widget for GbaScreen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let cols = area.width.min(CELL_WIDTH);
        let rows = area.height.min(CELL_HEIGHT);
        if cols == 0 || rows == 0 {
            return;
        }

        // Centre the visible window both on screen and within the image.
        let dst_x = area.x + (area.width - cols) / 2;
        let dst_y = area.y + (area.height - rows) / 2;
        let src_x = usize::from((CELL_WIDTH - cols) / 2);
        let src_y = usize::from((CELL_HEIGHT - rows) / 2);

        for row in 0..usize::from(rows) {
            let top = self.framebuffer.row((src_y + row) * 2);
            let bottom = self.framebuffer.row((src_y + row) * 2 + 1);
            let y = dst_y + row as u16;
            for col in 0..usize::from(cols) {
                let x = dst_x + col as u16;
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };
                let upper = top[src_x + col];
                let lower = bottom[src_x + col];
                cell.bg = rgba_to_color(lower);
                if upper == lower {
                    // Solid cell: a space with only a background keeps the
                    // terminal's escape output shorter.
                    cell.set_char(' ');
                    cell.fg = Color::Reset;
                } else {
                    cell.set_char(UPPER_HALF);
                    cell.fg = rgba_to_color(upper);
                }
            }
        }
    }
}

#[inline]
fn rgba_to_color(p: u32) -> Color {
    Color::Rgb((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8)
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
    fn smaller_area_centre_crops() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, 100, 40);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
        // Crop offset: x (240-100)/2 = 70, rows (80-40)/2 = 20 -> pixel y 40.
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(70, 40, 0));
        assert_eq!(buf[(99, 39)].bg, Color::Rgb(169, 119, 0));
        assert!(!GbaScreen::fits(area));
        assert!(GbaScreen::fits(Rect::new(0, 0, 240, 80)));
    }

    #[test]
    fn zero_area_is_a_no_op() {
        let fb = coordinate_frame();
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        GbaScreen::new(&fb).render(area, &mut buf);
    }
}
