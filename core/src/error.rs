//! Crate-wide error type.

/// Errors that can occur while loading or running the emulator.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GbaError {
    /// The ROM file could not be read from disk.
    #[error("failed to read ROM: {0}")]
    RomIo(#[from] std::io::Error),

    /// The ROM is larger than the 32 MiB cartridge address space.
    #[error("ROM is too large: {size} bytes (max {max} bytes)")]
    RomTooLarge {
        /// Actual size of the file.
        size: usize,
        /// Maximum size the cartridge bus can address.
        max: usize,
    },

    /// The ROM is too small to contain a cartridge header.
    #[error("ROM is too small: {size} bytes (need at least {min} bytes)")]
    RomTooSmall {
        /// Actual size of the file.
        size: usize,
        /// Minimum size for a valid header.
        min: usize,
    },

    /// The cartridge header failed validation.
    #[error("invalid ROM header: {0}")]
    InvalidHeader(String),

    /// A memory access hit an address with no mapped device.
    #[error("unmapped memory access at {address:#010x}")]
    UnmappedAddress {
        /// The faulting address.
        address: u32,
    },

    /// The CPU fetched an instruction it does not (yet) know how to execute.
    #[error("unimplemented {mode} instruction {opcode:#010x} at {pc:#010x}")]
    UnimplementedInstruction {
        /// `"ARM"` or `"THUMB"`.
        mode: &'static str,
        /// Raw instruction word (16-bit THUMB opcodes are zero-extended).
        opcode: u32,
        /// Address the instruction was fetched from.
        pc: u32,
    },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GbaError>;
