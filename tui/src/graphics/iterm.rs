//! iTerm2 inline images.
//!
//! The frame goes over as a base64 PNG inside an OSC 1337 sequence, sized
//! in cells so the terminal fits it into exactly the box we computed.
//!
//! Reference: <https://iterm2.com/documentation-images.html>

use std::io::{self, Write};

use tuiba_core::{Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};

use super::{Placement, base64_into, upscale_rgb};
use crate::png;

/// Encoder scratch, kept between frames.
#[derive(Debug, Default)]
pub(super) struct Iterm2 {
    pixels: Vec<u8>,
    png: Vec<u8>,
}

impl Iterm2 {
    /// Appends the escape that shows `fb` at the cursor.
    pub(super) fn encode(
        &mut self,
        fb: &Framebuffer,
        placement: Placement,
        out: &mut Vec<u8>,
    ) -> io::Result<()> {
        // A shrink is left to the terminal: it scales the native image
        // into the cell box.
        upscale_rgb(fb, placement.factor, &mut self.pixels);
        self.png.clear();
        #[allow(clippy::cast_possible_truncation)] // at most 1440 × 960
        png::write_rgb(
            &mut self.png,
            (SCREEN_WIDTH * placement.factor) as u32,
            (SCREEN_HEIGHT * placement.factor) as u32,
            &self.pixels,
        )?;
        write!(
            out,
            "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=1:",
            self.png.len(),
            placement.cols,
            placement.rows
        )?;
        base64_into(&self.png, out);
        out.push(0x07);
        Ok(())
    }
}
