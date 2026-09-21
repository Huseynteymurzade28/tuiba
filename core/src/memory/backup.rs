//! Cartridge backup memory: SRAM, flash and EEPROM, selected by the
//! save-type signature Nintendo's SDK embeds in every ROM.

use crate::memory::eeprom::Eeprom;

/// The kind of backup chip a cartridge carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveType {
    /// No signature found; behaves as 32 KiB SRAM so stray writes stick.
    Unknown,
    /// 32 KiB battery-backed SRAM.
    Sram,
    /// 64 KiB flash.
    Flash64K,
    /// 128 KiB flash in two banks.
    Flash128K,
    /// Serial EEPROM, mapped in the ROM area rather than at `0x0E00_0000`.
    Eeprom,
}

impl SaveType {
    /// Scans the ROM for the SDK's `xxx_Vnnn` save-type strings.
    #[must_use]
    pub fn detect(rom: &[u8]) -> Self {
        let has = |needle: &[u8]| rom.windows(needle.len()).any(|w| w == needle);
        if has(b"FLASH1M_V") {
            Self::Flash128K
        } else if has(b"FLASH512_V") || has(b"FLASH_V") {
            Self::Flash64K
        } else if has(b"SRAM_V") || has(b"SRAM_F_V") {
            Self::Sram
        } else if has(b"EEPROM_V") {
            Self::Eeprom
        } else {
            Self::Unknown
        }
    }
}

/// Flash command-sequence state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashState {
    /// Waiting for `0xAA` at `0x5555`.
    Idle,
    /// Got `0xAA`, waiting for `0x55` at `0x2AAA`.
    Unlock1,
    /// Got the unlock pair, waiting for the command byte at `0x5555`.
    Command,
    /// `0x80` seen: the next unlocked command is an erase.
    EraseUnlock1,
    EraseUnlock2,
    EraseCommand,
    /// `0xA0` seen: the next write programs one byte.
    Program,
    /// `0xB0` seen: the next write to `0x0000` selects the bank.
    BankSelect,
}

/// A flash chip with the common Sanyo/Macronix command set.
#[derive(Debug, Clone)]
pub struct Flash {
    data: Box<[u8]>,
    bank: usize,
    state: FlashState,
    id_mode: bool,
    manufacturer: u8,
    device: u8,
}

impl Flash {
    /// Sanyo LE26FV10N1TS, 128 KiB — the chip 1 Mbit carts expect.
    const ID_128K: (u8, u8) = (0x62, 0x13);
    /// Macronix MX29L512, 64 KiB.
    const ID_64K: (u8, u8) = (0xC2, 0x1C);

    /// An erased chip of `size` bytes (64 or 128 KiB).
    #[must_use]
    pub fn new(size: usize) -> Self {
        let (manufacturer, device) = if size > 0x1_0000 {
            Self::ID_128K
        } else {
            Self::ID_64K
        };
        Self {
            data: vec![0xFF; size].into_boxed_slice(),
            bank: 0,
            state: FlashState::Idle,
            id_mode: false,
            manufacturer,
            device,
        }
    }

    fn index(&self, address: u32) -> usize {
        (self.bank * 0x1_0000 + (address as usize & 0xFFFF)) % self.data.len()
    }

    /// Reads a byte; in ID mode the first two addresses return the chip ID.
    #[must_use]
    pub fn read(&self, address: u32) -> u8 {
        match (self.id_mode, address & 0xFFFF) {
            (true, 0) => self.manufacturer,
            (true, 1) => self.device,
            _ => self.data[self.index(address)],
        }
    }

    /// Feeds a byte write into the command state machine.
    pub fn write(&mut self, address: u32, value: u8) {
        let address = address & 0xFFFF;
        self.state = match (self.state, address, value) {
            (FlashState::Idle, 0x5555, 0xAA) => FlashState::Unlock1,
            (FlashState::Unlock1, 0x2AAA, 0x55) => FlashState::Command,
            (FlashState::Command, 0x5555, cmd) => match cmd {
                0x90 => {
                    self.id_mode = true;
                    FlashState::Idle
                }
                0xF0 => {
                    self.id_mode = false;
                    FlashState::Idle
                }
                0x80 => FlashState::EraseUnlock1,
                0xA0 => FlashState::Program,
                0xB0 => FlashState::BankSelect,
                _ => FlashState::Idle,
            },
            (FlashState::EraseUnlock1, 0x5555, 0xAA) => FlashState::EraseUnlock2,
            (FlashState::EraseUnlock2, 0x2AAA, 0x55) => FlashState::EraseCommand,
            (FlashState::EraseCommand, 0x5555, 0x10) => {
                self.data.fill(0xFF);
                FlashState::Idle
            }
            (FlashState::EraseCommand, sector, 0x30) if sector.trailing_zeros() >= 12 => {
                let start = self.index(sector);
                self.data[start..start + 0x1000].fill(0xFF);
                FlashState::Idle
            }
            (FlashState::Program, _, byte) => {
                // Programming can only clear bits, like real flash.
                let i = self.index(address);
                self.data[i] &= byte;
                FlashState::Idle
            }
            (FlashState::BankSelect, 0x0000, bank) => {
                self.bank = usize::from(bank & 1) % (self.data.len() / 0x1_0000).max(1);
                FlashState::Idle
            }
            // Any unexpected write resets the sequence.
            _ => FlashState::Idle,
        };
    }

    /// Raw contents, for saving.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Replaces the contents, e.g. from a save file. Extra bytes are
    /// ignored and missing ones stay erased.
    pub fn load(&mut self, data: &[u8]) {
        let n = data.len().min(self.data.len());
        self.data[..n].copy_from_slice(&data[..n]);
    }
}

/// The cartridge's backup device.
///
/// SRAM and flash answer byte accesses at `0x0E00_0000`; EEPROM instead
/// answers halfword accesses at `0x0D00_0000` (see [`Bus`](crate::Bus)).
#[derive(Debug, Clone)]
pub enum Backup {
    /// Plain byte-addressed RAM.
    Sram(Box<[u8]>),
    /// Command-driven flash.
    Flash(Flash),
    /// Bit-serial EEPROM.
    Eeprom(Eeprom),
}

impl Backup {
    /// The device for a save type.
    #[must_use]
    pub fn for_type(save_type: SaveType) -> Self {
        match save_type {
            SaveType::Flash64K => Self::Flash(Flash::new(0x1_0000)),
            SaveType::Flash128K => Self::Flash(Flash::new(0x2_0000)),
            SaveType::Eeprom => Self::Eeprom(Eeprom::new()),
            _ => Self::Sram(vec![0xFF; 0x8000].into_boxed_slice()),
        }
    }

    /// Reads a byte at `offset` within the SRAM region. EEPROM carts have
    /// nothing there and read back as open bus.
    #[must_use]
    pub fn read(&self, offset: u32) -> u8 {
        match self {
            Self::Sram(data) => data[offset as usize % data.len()],
            Self::Flash(flash) => flash.read(offset),
            Self::Eeprom(_) => 0xFF,
        }
    }

    /// Writes a byte at `offset` within the SRAM region.
    pub fn write(&mut self, offset: u32, value: u8) {
        match self {
            Self::Sram(data) => {
                let i = offset as usize % data.len();
                data[i] = value;
            }
            Self::Flash(flash) => flash.write(offset, value),
            Self::Eeprom(_) => {}
        }
    }

    /// The EEPROM chip, if this cartridge has one.
    #[must_use]
    pub fn eeprom(&self) -> Option<&Eeprom> {
        match self {
            Self::Eeprom(eeprom) => Some(eeprom),
            _ => None,
        }
    }

    /// Mutable access to the EEPROM chip, if this cartridge has one.
    pub fn eeprom_mut(&mut self) -> Option<&mut Eeprom> {
        match self {
            Self::Eeprom(eeprom) => Some(eeprom),
            _ => None,
        }
    }

    /// Raw contents, for saving to disk.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Sram(data) => data,
            Self::Flash(flash) => flash.data(),
            Self::Eeprom(eeprom) => eeprom.data(),
        }
    }

    /// Replaces the contents from a save file.
    pub fn load(&mut self, data: &[u8]) {
        match self {
            Self::Sram(sram) => {
                let n = data.len().min(sram.len());
                sram[..n].copy_from_slice(&data[..n]);
            }
            Self::Flash(flash) => flash.load(data),
            Self::Eeprom(eeprom) => eeprom.load(data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_save_type_strings() {
        assert_eq!(
            SaveType::detect(b"....FLASH1M_V103...."),
            SaveType::Flash128K
        );
        assert_eq!(
            SaveType::detect(b"....FLASH512_V131..."),
            SaveType::Flash64K
        );
        assert_eq!(SaveType::detect(b"....SRAM_V112..."), SaveType::Sram);
        assert_eq!(SaveType::detect(b"....EEPROM_V124..."), SaveType::Eeprom);
        assert_eq!(SaveType::detect(b"nothing here"), SaveType::Unknown);
    }

    fn unlock(flash: &mut Flash, command: u8) {
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, command);
    }

    #[test]
    fn id_mode_reports_chip() {
        let mut flash = Flash::new(0x2_0000);
        assert_eq!(flash.read(0), 0xFF);
        unlock(&mut flash, 0x90);
        assert_eq!((flash.read(0), flash.read(1)), (0x62, 0x13));
        unlock(&mut flash, 0xF0);
        assert_eq!(flash.read(0), 0xFF);
        assert_eq!(
            (
                Flash::new(0x1_0000).manufacturer,
                Flash::new(0x1_0000).device
            ),
            (0xC2, 0x1C)
        );
    }

    #[test]
    fn program_erase_and_banks() {
        let mut flash = Flash::new(0x2_0000);
        unlock(&mut flash, 0xA0);
        flash.write(0x1234, 0x5A);
        assert_eq!(flash.read(0x1234), 0x5A);
        unlock(&mut flash, 0xA0);
        flash.write(0x1234, 0x0F);
        assert_eq!(flash.read(0x1234), 0x0A, "program only clears bits");

        // Bank 1 is separate storage.
        unlock(&mut flash, 0xB0);
        flash.write(0x0000, 1);
        assert_eq!(flash.read(0x1234), 0xFF);
        unlock(&mut flash, 0xA0);
        flash.write(0x1234, 0x77);
        assert_eq!(flash.data()[0x1_1234], 0x77);
        unlock(&mut flash, 0xB0);
        flash.write(0x0000, 0);
        assert_eq!(flash.read(0x1234), 0x0A);

        // Sector erase clears 4 KiB, chip erase everything.
        unlock(&mut flash, 0x80);
        unlock(&mut flash, 0x30_u8);
        // The sector-erase command byte goes to the sector address instead.
        let mut flash2 = Flash::new(0x2_0000);
        unlock(&mut flash2, 0xA0);
        flash2.write(0x1000, 0x00);
        unlock(&mut flash2, 0xA0);
        flash2.write(0x2000, 0x00);
        unlock(&mut flash2, 0x80);
        flash2.write(0x5555, 0xAA);
        flash2.write(0x2AAA, 0x55);
        flash2.write(0x1000, 0x30);
        assert_eq!(flash2.read(0x1000), 0xFF, "sector erased");
        assert_eq!(flash2.read(0x2000), 0x00, "other sector intact");
        unlock(&mut flash2, 0x80);
        flash2.write(0x5555, 0xAA);
        flash2.write(0x2AAA, 0x55);
        flash2.write(0x5555, 0x10);
        assert_eq!(flash2.read(0x2000), 0xFF, "chip erased");
    }

    #[test]
    fn sram_backup_round_trips_and_loads() {
        let mut backup = Backup::for_type(SaveType::Sram);
        backup.write(0x10, 0x42);
        assert_eq!(backup.read(0x10), 0x42);
        assert_eq!(backup.read(0x8010), 0x42, "32 KiB mirror");
        let mut other = Backup::for_type(SaveType::Sram);
        other.load(backup.data());
        assert_eq!(other.read(0x10), 0x42);
    }
}
