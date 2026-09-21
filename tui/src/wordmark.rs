//! The `TUIBA` wordmark drawn with block characters.
//!
//! Letters come from a 7-row pixel font with two-pixel strokes. A
//! terminal cell is about twice as tall as it is wide, so a half block
//! (`▀`/`▄`) is a square pixel: one column per pixel, two pixel rows per
//! text row, four text rows in all.

use ratatui::style::Style;
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

/// Width in terminal columns.
#[must_use]
pub fn width() -> u16 {
    pixel_rows()[0].len() as u16
}

/// The wordmark as styled lines. `shades` colours the text rows from
/// top to bottom (the last entry repeats if there are fewer than four).
#[must_use]
pub fn lines(shades: &[Style]) -> Vec<Line<'static>> {
    let rows = pixel_rows();
    let width = rows[0].len();
    (0..HEIGHT as usize)
        .map(|text_row| {
            let style = shades[text_row.min(shades.len() - 1)];
            let upper = &rows[text_row * 2];
            let lower = rows.get(text_row * 2 + 1);
            let mut line = String::with_capacity(width * 3);
            for x in 0..width {
                line.push(match (upper[x], lower.is_some_and(|r| r[x])) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
            Line::from(Span::styled(line, style))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_five_letters_in_four_rows() {
        let lines = lines(&[Style::default()]);
        assert_eq!(lines.len(), 4);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(text[0].chars().count() as u16, width());
        assert_eq!(width(), 30);
        // The T: a two-pixel bar fills the first text row.
        assert!(text[0].starts_with("██████ "), "{}", text[0]);
        // The last row holds only the seventh pixel row: upper halves.
        assert!(!text[3].contains('▄'));
        assert!(text[3].contains('▀'));
    }
}
