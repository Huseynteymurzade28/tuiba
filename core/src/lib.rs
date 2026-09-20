//! Game Boy Advance emulator core.
//!
//! This crate is completely frontend-agnostic: it knows nothing about
//! terminals, windows or audio devices. A frontend drives the emulator by
//! stepping it, feeding it input state and reading back the framebuffer.
//!
//! # Module layout
//!
//! - [`error`]  – the crate-wide [`GbaError`] type.
//! - [`memory`] – GBA address-space layout, bus and cartridge.
//! - [`cpu`]    – the ARM7TDMI core.
//! - [`ppu`]    – LCD timing and scanline rendering into a framebuffer.

pub mod cpu;
pub mod error;
pub mod memory;
pub mod ppu;

pub use cpu::Cpu;
pub use error::{GbaError, Result};
pub use memory::{Bus, Cartridge, Memory};
pub use ppu::{Framebuffer, Ppu, SCREEN_HEIGHT, SCREEN_WIDTH};
