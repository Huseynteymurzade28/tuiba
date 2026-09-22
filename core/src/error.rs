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

    /// A BIOS image was not exactly 16 KiB.
    #[error("BIOS image is {size} bytes, expected {expected}")]
    BiosSize {
        /// Actual size of the image.
        size: usize,
        /// Required size.
        expected: usize,
    },

    /// The cartridge header failed validation.
    #[error("invalid ROM header: {0}")]
    InvalidHeader(String),

    /// The bytes handed to [`Snapshot::from_bytes`](crate::Snapshot::from_bytes)
    /// are not a save state at all.
    #[error("not a tuiba save state")]
    StateFormat,

    /// The save state was written by a tuiba whose state layout differs
    /// from this one's.
    #[error("save state has format version {found}, this build reads {expected}")]
    StateVersion {
        /// Version stamped into the file.
        found: u16,
        /// Version this build writes and reads.
        expected: u16,
    },

    /// The save state was taken from a different cartridge than the one
    /// running. Restoring it would put one game's memory behind another
    /// game's code.
    #[error("save state belongs to a different cartridge")]
    StateCartridge,

    /// The save state is the right shape but its contents did not decode.
    #[error("save state is corrupt: {0}")]
    StateCorrupt(String),

    /// A memory access hit an address with no mapped device.
    #[error("unmapped memory access at {address:#010x}")]
    UnmappedAddress {
        /// The faulting address.
        address: u32,
    },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GbaError>;
