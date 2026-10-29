//! The `TUIBA` wordmark drawn with block characters.
//!
//! Letters come from a 7-row pixel font with two-pixel strokes. A
//! terminal cell is about twice as tall as it is wide, so a half block
//! (`▀`/`▄`) is a square pixel: one column per pixel, two pixel rows per
//! text row, four text rows in all, the last one shared with a drop
//! shadow.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Pixel rows of each letter, `#` = lit.
const FONT: [[&str; 7]; 5] = [
    // T
    [
        "######", "######", "  ##  ", "  ##  ", "  ##  ", "  ##  ", "  ##  ",
    ],
    // U
    [
        "##  ##", "##  ##", "##  ##", "##  ##", "##  ##", "##  ##", " #### ",
    ],
    // I
    ["##", "##", "##", "##", "##", "##", "##"],
    // B
    [
        "##### ", "##  ##", "##  ##", "##### ", "##  ##", "##  ##", "##### ",
    ],
    // A
    [
        " #### ", "##  ##", "##  ##", "######", "##  ##", "##  ##", "##  ##",
    ],
];

/// Columns between letters, in pixels.
const LETTER_GAP: usize = 1;

/// Height in terminal rows.
pub const HEIGHT: u16 = 4;

/// Pixel rows of the whole word.
fn pixel_rows() -> Vec<Vec<bool>> {
    let mut rows: Vec<Vec<bool>> = vec![Vec::new(); 7];
    for (i, glyph) in FONT.iter().enumerate() {
        for (r, row) in glyph.iter().enumerate() {
            if i > 0 {
                rows[r].extend(std::iter::repeat_n(false, LETTER_GAP));
            }
            rows[r].extend(row.chars().map(|c| c == '#'));
        }
    }
    rows
}

/// Width in terminal columns, the drop shadow included.
#[must_use]
pub fn width() -> u16 {
    pixel_rows()[0].len() as u16 + SHADOW
}

/// How far the drop shadow falls, right and down, in pixels.
const SHADOW: u16 = 1;

/// The wordmark as styled lines, every lit pixel coloured by
/// `paint(x, y)` (in pixels from the top left) and the drop shadow in
/// `shadow`. Each cell carries two pixels, the upper one as the
/// foreground of `▀` and the lower one as its background, so colours
/// change at pixel rather than text-row resolution.
#[must_use]
pub fn painted(paint: impl Fn(u16, u16) -> Color, shadow: Color) -> Vec<Line<'static>> {
    let rows = pixel_rows();
    let lit = |x: u16, y: u16| {
        rows.get(usize::from(y))
            .and_then(|r| r.get(usize::from(x)))
            .copied()
            .unwrap_or(false)
    };
    let offset = SHADOW;
    let pixel = |x: u16, y: u16| {
        if lit(x, y) {
            Some(paint(x, y))
        } else if x >= offset && y >= offset && lit(x - offset, y - offset) {
            Some(shadow)
        } else {
            None
        }
    };
    (0..HEIGHT)
        .map(|text_row| {
            let spans: Vec<Span<'static>> = (0..width())
                .map(
                    |x| match (pixel(x, text_row * 2), pixel(x, text_row * 2 + 1)) {
                        (Some(upper), Some(lower)) => {
                            Span::styled("▀", Style::default().fg(upper).bg(lower))
                        }
                        (Some(upper), None) => Span::styled("▀", Style::default().fg(upper)),
                        (None, Some(lower)) => Span::styled("▄", Style::default().fg(lower)),
                        (None, None) => Span::raw(" "),
                    },
                )
                .collect();
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_five_letters_in_four_rows() {
        let lines = painted(|_, _| Color::White, Color::DarkGray);
        assert_eq!(lines.len(), 4);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(text[0].chars().count() as u16, width());
        assert_eq!(width(), 31);
        // The T: a two-pixel bar fills the first text row.
        assert!(text[0].starts_with("▀▀▀▀▀▀▄"), "{}", text[0]);
        // The last row holds the seventh pixel row and the shadow below it.
        assert!(text[3].contains('▀'));
    }

    #[test]
    fn colours_each_pixel_and_casts_a_shadow() {
        let lines = painted(
            |_, y| if y == 0 { Color::Red } else { Color::Blue },
            Color::Black,
        );
        // Top-left cell of the T: red over blue.
        let first = &lines[0].spans[0];
        assert_eq!(first.style.fg, Some(Color::Red));
        assert_eq!(first.style.bg, Some(Color::Blue));
        // Right of the T's bar, one pixel down: shadow.
        let edge = &lines[0].spans[6];
        assert_eq!(edge.content, "▄");
        assert_eq!(edge.style.fg, Some(Color::Black));
    }
}
