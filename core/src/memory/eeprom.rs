//! Serial EEPROM backup memory.
//!
//! The chip sits in the top of the ROM address space (`0x0D00_0000`) and
//! talks one bit at a time: software writes a stream of halfwords whose
//! bit 0 forms a request, then reads halfwords back to get the reply.
//! Requests are `10` + address + 64 data bits + a stop bit (write), or
//! `11` + address + stop bit (read); a read reply is 4 dummy bits followed
//! by the 64 data bits, MSB first. Data is addressed in 8-byte blocks.
//!
//! Two sizes exist and cannot be told apart from the ROM: 512 B chips take
//! 6-bit addresses, 8 KiB chips 14-bit. Games always drive the chip with
//! DMA3, so the length of the first request transfer reveals which one
//! the game expects (see [`Eeprom::hint_transfer_len`]).

use std::cell::Cell;

/// Largest supported chip.
const MAX_SIZE: usize = 0x2000;
/// Size of the small chip.
const SMALL_SIZE: usize = 0x200;
/// Bits in a reply: 4 dummy bits plus one 8-byte block.
const REPLY_BITS: u8 = 4 + 64;

/// Where the request parser is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    /// Collecting the 2-bit command.
    Command { bits: u8, count: u8 },
    /// Collecting address bits.
    Address { read: bool, address: u16, count: u8 },
    /// Collecting the 64 data bits of a write.
    WriteData { address: u16, data: u64, count: u8 },
    /// Waiting for the stop bit that ends a write request.
    WriteStop { address: u16, data: u64 },
    /// Waiting for the stop bit that ends a read request.
    ReadStop { address: u16 },
    /// Serving a reply; the bit position lives in `Eeprom::reply_pos`.
    Reply { address: u16 },
}

impl State {
    const IDLE: Self = Self::Command { bits: 0, count: 0 };
}

/// An EEPROM chip of either size.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Eeprom {
    /// Always `MAX_SIZE` long; only the first `size()` bytes are used.
    data: Box<[u8]>,
    /// `Some(6)` or `Some(14)` once the chip size is known.
    address_bits: Option<u8>,
    state: State,
    /// Bits of the reply already returned. Reads take `&self`, hence the
    /// cell.
    reply_pos: Cell<u8>,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self::new()
    }
}

impl Eeprom {
    /// An erased chip of yet-unknown size.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: vec![0xFF; MAX_SIZE].into_boxed_slice(),
            address_bits: None,
            state: State::IDLE,
            reply_pos: Cell::new(REPLY_BITS),
        }
    }

    /// The chip size in bytes; the large one until proven otherwise.
    #[must_use]
    pub fn size(&self) -> usize {
        if self.address_bits == Some(6) {
            SMALL_SIZE
        } else {
            MAX_SIZE
        }
    }

    /// Learns the chip size from the length (in halfwords) of a DMA to or
    /// from the chip. Read requests are 9 or 17 units, write requests 73
    /// or 81; other lengths (such as the 68-unit reply fetch) say nothing.
    /// Only the first informative transfer counts.
    pub fn hint_transfer_len(&mut self, units: u32) {
        if self.address_bits.is_some() {
            return;
        }
        self.address_bits = match units {
            9 | 73 => Some(6),
            17 | 81 => Some(14),
            _ => None,
        };
    }

    /// Address bits to expect; assumes the large chip if unknown.
    fn address_bits(&self) -> u8 {
        self.address_bits.unwrap_or(14)
    }

    /// Byte offset of an 8-byte block, wrapped to the chip.
    fn block(&self, address: u16) -> usize {
        (usize::from(address) * 8) % self.size()
    }

    /// Feeds one request bit (bit 0 of a halfword write).
    pub fn write(&mut self, value: u16) {
        let bit = value & 1;
        self.state = match self.state {
            // A write interrupts any reply in progress.
            State::Reply { .. } | State::Command { .. } => {
                let (bits, count) = match self.state {
                    State::Command { bits, count } => (bits, count),
                    _ => (0, 0),
                };
                let bits = (bits << 1) | bit as u8;
                match (count + 1, bits) {
                    (2, 0b10) => State::Address {
                        read: false,
                        address: 0,
                        count: 0,
                    },
                    (2, 0b11) => State::Address {
                        read: true,
                        address: 0,
                        count: 0,
                    },
                    // `0x` is not a command; keep looking for a leading 1.
                    (2, _) => State::IDLE,
                    (count, bits) => State::Command { bits, count },
                }
            }
            State::Address {
                read,
                address,
                count,
            } => {
                let address = (address << 1) | bit;
                let count = count + 1;
                if count < self.address_bits() {
                    State::Address {
                        read,
                        address,
                        count,
                    }
                } else if read {
                    State::ReadStop { address }
                } else {
                    State::WriteData {
                        address,
                        data: 0,
                        count: 0,
                    }
                }
            }
            State::WriteData {
                address,
                data,
                count,
            } => {
                let data = (data << 1) | u64::from(bit);
                if count + 1 < 64 {
                    State::WriteData {
                        address,
                        data,
                        count: count + 1,
                    }
                } else {
                    State::WriteStop { address, data }
                }
            }
            State::WriteStop { address, data } => {
                let start = self.block(address);
                self.data[start..start + 8].copy_from_slice(&data.to_be_bytes());
                State::IDLE
            }
            State::ReadStop { address } => {
                self.reply_pos.set(0);
                State::Reply { address }
            }
        };
    }

    /// Returns the next reply bit, or `1` ("ready") when there is nothing
    /// to reply.
    #[must_use]
    pub fn read(&self) -> u16 {
        let State::Reply { address } = self.state else {
            return 1;
        };
        let pos = self.reply_pos.get();
        if pos >= REPLY_BITS {
            return 1;
        }
        self.reply_pos.set(pos + 1);
        if pos < 4 {
            return 0;
        }
        let start = self.block(address);
        let block = &self.data[start..start + 8];
        let bit = pos - 4;
        u16::from(block[usize::from(bit / 8)] >> (7 - bit % 8)) & 1
    }

    /// The chip contents, for saving.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data[..self.size()]
    }

    /// Restores the contents from a save file, whose length also settles
    /// the chip size.
    pub fn load(&mut self, data: &[u8]) {
        if self.address_bits.is_none() {
            self.address_bits = Some(if data.len() <= SMALL_SIZE { 6 } else { 14 });
        }
        let n = data.len().min(self.size());
        self.data[..n].copy_from_slice(&data[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(eeprom: &mut Eeprom, bits: &str) {
        for c in bits.chars().filter(|c| !c.is_whitespace()) {
            eeprom.write(u16::from(c == '1'));
        }
    }

    fn address(bits: u8, value: u16) -> String {
        (0..bits)
            .rev()
            .map(|i| if value >> i & 1 == 1 { '1' } else { '0' })
            .collect()
    }

    fn write_block(eeprom: &mut Eeprom, bits: u8, address_value: u16, data: u64) {
        send(eeprom, "10");
        send(eeprom, &address(bits, address_value));
        send(eeprom, &format!("{data:064b}"));
        send(eeprom, "0");
    }

    fn read_block(eeprom: &mut Eeprom, bits: u8, address_value: u16) -> u64 {
        send(eeprom, "11");
        send(eeprom, &address(bits, address_value));
        send(eeprom, "0");
        for _ in 0..4 {
            assert_eq!(eeprom.read(), 0, "dummy bits");
        }
        (0..64).fold(0, |acc, _| (acc << 1) | u64::from(eeprom.read()))
    }

    #[test]
    fn idle_reads_ready() {
        let eeprom = Eeprom::new();
        assert_eq!(eeprom.read(), 1);
    }

    #[test]
    fn write_then_read_large_chip() {
        let mut eeprom = Eeprom::new();
        eeprom.hint_transfer_len(81);
        assert_eq!(eeprom.size(), MAX_SIZE);
        write_block(&mut eeprom, 14, 0x3FF, 0x0123_4567_89AB_CDEF);
        assert_eq!(
            &eeprom.data()[0x1FF8..],
            &0x0123_4567_89AB_CDEF_u64.to_be_bytes()
        );
        assert_eq!(eeprom.read(), 1, "ready after write");
        assert_eq!(read_block(&mut eeprom, 14, 0x3FF), 0x0123_4567_89AB_CDEF);
        assert_eq!(eeprom.read(), 1, "ready after reply");
        assert_eq!(read_block(&mut eeprom, 14, 0), u64::MAX, "erased block");
    }

    #[test]
    fn small_chip_uses_six_bit_addresses() {
        let mut eeprom = Eeprom::new();
        eeprom.hint_transfer_len(68); // a reply fetch: uninformative
        assert_eq!(eeprom.address_bits, None);
        eeprom.hint_transfer_len(9);
        assert_eq!(eeprom.size(), SMALL_SIZE);
        eeprom.hint_transfer_len(17); // later hints don't flip the size
        assert_eq!(eeprom.size(), SMALL_SIZE);

        write_block(&mut eeprom, 6, 63, 0xDEAD_BEEF_0000_0001);
        assert_eq!(read_block(&mut eeprom, 6, 63), 0xDEAD_BEEF_0000_0001);
        assert_eq!(eeprom.data().len(), SMALL_SIZE);
    }

    #[test]
    fn stray_zero_bits_do_not_start_a_command() {
        let mut eeprom = Eeprom::new();
        eeprom.hint_transfer_len(17);
        send(&mut eeprom, "00 01");
        write_block(&mut eeprom, 14, 1, 42);
        assert_eq!(read_block(&mut eeprom, 14, 1), 42);
    }

    #[test]
    fn save_file_length_sets_size() {
        let mut eeprom = Eeprom::new();
        eeprom.load(&[0x11; SMALL_SIZE]);
        assert_eq!(eeprom.size(), SMALL_SIZE);
        assert_eq!(read_block(&mut eeprom, 6, 2), 0x1111_1111_1111_1111);

        let mut large = Eeprom::new();
        large.load(&[0x22; MAX_SIZE]);
        assert_eq!(large.size(), MAX_SIZE);
        assert_eq!(read_block(&mut large, 14, 1023), 0x2222_2222_2222_2222);
    }
}
