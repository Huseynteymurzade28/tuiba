//! The `TUIBA` wordmark drawn with block characters.
//!
//! Letters come from a 5-row pixel font. Terminal cells are about twice
//! as tall as they are wide, so every pixel is two columns wide and half
//! a row tall (`▀`/`▄`/`█`), which keeps the letters' proportions.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Pixel rows of each letter, `#` = lit.
const FONT: [(&str, [&str; 5]); 5] = [
    ("T", ["#####", "  #  ", "  #  ", "  #  ", "  #  "]),
    ("U", ["#   #", "#   #", "#   #", "#   #", " ### "]),
    ("I", ["###", " # ", " # ", " # ", "###"]),
    ("B", ["#### ", "#   #", "#### ", "#   #", "#### "]),
    ("A", [" ### ", "#   #", "#####", "#   #", "#   #"]),
];

/// Columns between letters, in pixels.
const LETTER_GAP: usize = 1;

/// Pixel rows of the whole word.
fn pixel_rows() -> Vec<Vec<bool>> {
    let mut rows: Vec<Vec<bool>> = vec![Vec::new(); 5];
    for (i, (_, glyph)) in FONT.iter().enumerate() {
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
    (pixel_rows()[0].len() * 2) as u16
}

/// Height in terminal rows.
pub const HEIGHT: u16 = 3;

/// The wordmark as styled lines; `top` colours the upper pixel rows,
/// `bottom` the lower ones, for a little depth.
#[must_use]
pub fn lines(top: Style, bottom: Style) -> Vec<Line<'static>> {
    let rows = pixel_rows();
    let width = rows[0].len();
    (0..HEIGHT as usize)
        .map(|text_row| {
            let upper = &rows[text_row * 2];
            let lower = rows.get(text_row * 2 + 1);
            let mut spans = Vec::with_capacity(width);
            for x in 0..width {
                let (u, l) = (upper[x], lower.is_some_and(|r| r[x]));
                let (glyph, style) = match (u, l) {
                    (true, true) => ("██", if text_row == 0 { top } else { bottom }),
                    (true, false) => ("▀▀", if text_row == 0 { top } else { bottom }),
                    (false, true) => ("▄▄", bottom),
                    (false, false) => ("  ", bottom),
                };
                spans.push(Span::styled(glyph, style));
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_five_letters_in_three_rows() {
        let lines = lines(Style::default(), Style::default());
        assert_eq!(lines.len(), 3);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(text[0].chars().count() as u16, width());
        // The T: a half-height bar with a full-height stem in the middle.
        assert!(text[0].starts_with("▀▀▀▀██▀▀▀▀"), "{}", text[0]);
        // The bottom row only has upper halves (row 4 of the font).
        assert!(!text[2].contains('▄'));
        assert!(text[2].contains("▀▀"));
    }
}
