//! Game Boy Advance emulator core.
//!
//! This crate is completely frontend-agnostic: it knows nothing about
//! terminals, windows or audio devices. A frontend drives the emulator by
//! stepping it, feeding it input state and reading back the framebuffer.
//!
//! # Module layout
//!
//! - [`error`]  – the crate-wide [`GbaError`] type.
//! - [`memory`] – GBA address-space layout and (later) the memory bus.

pub mod error;
pub mod memory;

pub use error::{GbaError, Result};
pub use memory::{Bus, Cartridge};
