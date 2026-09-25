//! The cartridge GPIO port and the real-time clock behind it.
//!
//! Some cartridges wire a Seiko S-3511 clock chip to four general-purpose
//! pins exposed as three halfword registers in the ROM area:
//!
//! | Offset  | Register  | Bits                                            |
//! | ------- | --------- | ----------------------------------------------- |
//! | `0xC4`  | data      | 0 SCK, 1 SIO, 2 CS, 3 unused                    |
//! | `0xC6`  | direction | 1 = the GBA drives that pin                     |
//! | `0xC8`  | control   | bit 0: the registers read back (else ROM bytes) |
//!
//! The game bit-bangs a serial protocol over the pins: raising CS starts
//! a transfer, a command byte follows most significant bit first, then
//! parameter bytes least significant bit first, each bit clocked by a
//! rising edge on SCK. Dropping CS ends it.
//!
//! The port is always there: a cartridge without the chip never writes
//! to these offsets, and reads keep returning ROM bytes until a game
//! switches the control bit on. So nothing has to be detected up front.
//!
//! The clock itself is not kept here. The frontend hands the current
//! date and time in with [`Gpio::set_clock`]; a transfer latches it.
//! Setting the clock from the game is accepted and ignored, as the
//! calendar is the host's.

/// Offset of the data register from the start of the ROM.
pub const DATA: u32 = 0xC4;
/// Offset of the direction register.
pub const DIRECTION: u32 = 0xC6;
/// Offset of the control register.
pub const CONTROL: u32 = 0xC8;

const SCK: u8 = 1 << 0;
const SIO: u8 = 1 << 1;
const CS: u8 = 1 << 2;

/// A calendar date and time of day, as the clock chip reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DateTime {
    /// Full year; the chip counts 2000–2099.
    pub year: u16,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: u8,
    /// 0–6, counting from Sunday.
    pub weekday: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
    /// 0–59.
    pub second: u8,
}

impl Default for DateTime {
    /// Midnight on 1 January 2000, a Saturday: where the chip starts
    /// counting.
    fn default() -> Self {
        Self {
            year: 2000,
            month: 1,
            day: 1,
            weekday: 6,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

/// S-3511 commands, from bits 1–3 of the command byte.
mod command {
    pub const RESET: u8 = 0;
    pub const STATUS: u8 = 1;
    pub const DATE_TIME: u8 = 2;
    pub const TIME: u8 = 3;
}

/// Status register bit: hours count 0–23 rather than 0–11 with a PM flag.
const STATUS_24H: u8 = 1 << 6;

/// Parameter bytes that follow each command.
const fn parameter_bytes(command: u8) -> u8 {
    match command {
        command::STATUS => 1,
        command::DATE_TIME => 7,
        command::TIME => 3,
        _ => 0,
    }
}

/// Where a transfer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
enum Phase {
    /// CS is low.
    #[default]
    Idle,
    /// Shifting in the command byte.
    Command,
    /// Moving parameter bytes of `command`, in or out.
    Parameters { command: u8, read: bool },
    /// The command's bytes are all done; waiting for CS to drop.
    Done,
}

/// The S-3511 real-time clock.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Rtc {
    phase: Phase,
    /// Bits of the byte being shifted, and how many have arrived.
    shift: u8,
    bits: u8,
    /// Parameter byte the transfer is on.
    index: u8,
    /// The command's parameters: the latched date and time, or the status.
    buffer: [u8; 7],
    /// The chip's status register.
    status: u8,
    /// What the chip drives onto SIO while it is sending.
    sio: bool,
}

impl Default for Rtc {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            shift: 0,
            bits: 0,
            index: 0,
            buffer: [0; 7],
            // The chip powers up in 24-hour mode as far as games can
            // tell: they write it on boot anyway.
            status: STATUS_24H,
            sio: false,
        }
    }
}

impl Rtc {
    /// Reacts to the pins changing from `before` to `after`.
    fn pins(&mut self, before: u8, after: u8, now: DateTime) {
        if after & CS == 0 {
            self.phase = Phase::Idle;
            return;
        }
        if before & CS == 0 {
            self.phase = Phase::Command;
            self.shift = 0;
            self.bits = 0;
            return;
        }
        // Everything else happens on a rising edge of the clock.
        if before & SCK != 0 || after & SCK == 0 {
            return;
        }
        let bit = after & SIO != 0;
        match self.phase {
            Phase::Idle | Phase::Done => {}
            Phase::Command => {
                self.shift = (self.shift << 1) | u8::from(bit);
                self.bits += 1;
                if self.bits == 8 {
                    self.command(self.shift, now);
                }
            }
            Phase::Parameters {
                read: true,
                command,
            } => {
                self.sio = self.buffer[usize::from(self.index)] >> self.bits & 1 != 0;
                self.next_bit(command);
            }
            Phase::Parameters {
                read: false,
                command,
            } => {
                self.shift |= u8::from(bit) << self.bits;
                if self.bits == 7 {
                    self.buffer[usize::from(self.index)] = self.shift;
                    if command == command::STATUS {
                        self.status = self.shift;
                    }
                }
                self.next_bit(command);
            }
        }
    }

    /// Counts off a parameter bit, moving to the next byte after eight
    /// and finishing after the command's last.
    fn next_bit(&mut self, command: u8) {
        self.bits += 1;
        if self.bits < 8 {
            return;
        }
        self.bits = 0;
        self.shift = 0;
        self.index += 1;
        if self.index >= parameter_bytes(command) {
            self.phase = Phase::Done;
        }
    }

    /// Starts on a command byte: `0110 ccc r`, command `ccc`, `r` set
    /// when the game reads the parameters rather than writing them.
    fn command(&mut self, byte: u8, now: DateTime) {
        self.bits = 0;
        self.shift = 0;
        self.index = 0;
        if byte >> 4 != 0b0110 {
            // Not addressed to this chip; stay quiet until CS drops.
            self.phase = Phase::Done;
            return;
        }
        let command = (byte >> 1) & 7;
        let read = byte & 1 != 0;
        match command {
            command::RESET => self.status = 0,
            command::STATUS => self.buffer[0] = self.status,
            command::DATE_TIME | command::TIME => self.latch(now, command),
            _ => {}
        }
        self.phase = if parameter_bytes(command) == 0 {
            Phase::Done
        } else {
            Phase::Parameters { command, read }
        };
    }

    /// Copies the clock into the buffer in the chip's BCD layout; a
    /// time-only read starts at the hour.
    fn latch(&mut self, now: DateTime, command: u8) {
        let hour = if self.status & STATUS_24H != 0 {
            bcd(now.hour)
        } else {
            bcd(now.hour % 12) | if now.hour >= 12 { 0x80 } else { 0 }
        };
        let date_time = [
            bcd((now.year % 100) as u8),
            bcd(now.month),
            bcd(now.day),
            now.weekday,
            hour,
            bcd(now.minute),
            bcd(now.second),
        ];
        self.buffer = date_time;
        if command == command::TIME {
            self.buffer.copy_within(4.., 0);
        }
    }
}

/// Binary to packed BCD, for values below 100.
const fn bcd(n: u8) -> u8 {
    ((n / 10) << 4) | (n % 10)
}

/// The GPIO port with the clock on it.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Gpio {
    /// Pin levels as last written by the game.
    data: u8,
    /// Which pins the game drives.
    direction: u8,
    /// Whether the registers read back.
    readable: bool,
    rtc: Rtc,
    /// What the clock reads, as the frontend last set it.
    clock: DateTime,
}

impl Gpio {
    /// Sets what the clock will report the next time a game reads it.
    pub fn set_clock(&mut self, now: DateTime) {
        self.clock = now;
    }

    /// Reads a register, or `None` when the port is not readable (or
    /// `offset` is not one of its registers) and the ROM shows through.
    #[must_use]
    pub fn read16(&self, offset: u32) -> Option<u16> {
        if !self.readable {
            return None;
        }
        match offset {
            DATA => {
                let from_chip = if self.rtc.sio { SIO } else { 0 };
                let pins = (self.data & self.direction) | (from_chip & !self.direction);
                Some(u16::from(pins))
            }
            DIRECTION => Some(u16::from(self.direction)),
            CONTROL => Some(u16::from(self.readable)),
            _ => None,
        }
    }

    /// Writes a register. Offsets that are not registers are ignored,
    /// like every other write to ROM.
    pub fn write16(&mut self, offset: u32, value: u16) {
        let value = value as u8 & 0xF;
        match offset {
            DATA => {
                let before = self.data;
                self.data = (self.data & !self.direction) | (value & self.direction);
                self.rtc.pins(before, self.data, self.clock);
            }
            DIRECTION => self.direction = value,
            CONTROL => self.readable = value & 1 != 0,
            _ => {}
        }
    }

    /// Whether a ROM offset falls on the port.
    #[must_use]
    pub const fn covers(offset: u32) -> bool {
        matches!(offset, DATA..=0xC9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the port the way Nintendo's RTC library does.
    struct Game<'a>(&'a mut Gpio);

    impl Game<'_> {
        fn pins(&mut self, pins: u8) {
            self.0.write16(DATA, u16::from(pins));
        }

        fn begin(&mut self) {
            self.0.write16(CONTROL, 1);
            self.pins(SCK);
            self.pins(SCK | CS);
            self.0.write16(DIRECTION, 0b0111);
        }

        fn end(&mut self) {
            self.pins(SCK);
        }

        fn send(&mut self, bits: impl Iterator<Item = bool>) {
            for bit in bits {
                let sio = if bit { SIO } else { 0 };
                self.pins(sio | CS);
                self.pins(sio | SCK | CS);
            }
        }

        fn command(&mut self, byte: u8) {
            self.send((0..8).rev().map(|i| byte >> i & 1 != 0));
        }

        fn write(&mut self, byte: u8) {
            self.send((0..8).map(|i| byte >> i & 1 != 0));
        }

        fn read(&mut self) -> u8 {
            self.0.write16(DIRECTION, 0b0101);
            let mut value = 0;
            for i in 0..8 {
                self.pins(CS);
                self.pins(SCK | CS);
                let sio = self.0.read16(DATA).expect("readable") as u8 & SIO;
                value |= (sio >> 1) << i;
            }
            self.0.write16(DIRECTION, 0b0111);
            value
        }

        fn transfer_read(&mut self, command: u8, count: usize) -> Vec<u8> {
            self.begin();
            self.command(command);
            let bytes = (0..count).map(|_| self.read()).collect();
            self.end();
            bytes
        }
    }

    fn afternoon() -> DateTime {
        DateTime {
            year: 2026,
            month: 9,
            day: 25,
            weekday: 5,
            hour: 21,
            minute: 7,
            second: 42,
        }
    }

    #[test]
    fn reads_the_date_and_time_in_bcd() {
        let mut gpio = Gpio::default();
        gpio.set_clock(afternoon());
        let bytes = Game(&mut gpio).transfer_read(0x65, 7);
        assert_eq!(bytes, [0x26, 0x09, 0x25, 5, 0x21, 0x07, 0x42]);
    }

    #[test]
    fn reads_the_time_alone() {
        let mut gpio = Gpio::default();
        gpio.set_clock(afternoon());
        let bytes = Game(&mut gpio).transfer_read(0x67, 3);
        assert_eq!(bytes, [0x21, 0x07, 0x42]);
    }

    #[test]
    fn status_is_written_and_read_back_and_reset_clears_it() {
        let mut gpio = Gpio::default();
        let mut game = Game(&mut gpio);
        assert_eq!(game.transfer_read(0x63, 1), [STATUS_24H]);

        game.begin();
        game.command(0x60);
        game.end();
        assert_eq!(game.transfer_read(0x63, 1), [0]);

        // Twelve-hour mode: 21:07 reads as 9 with the PM flag.
        game.0.set_clock(afternoon());
        assert_eq!(game.transfer_read(0x67, 1), [0x89]);

        game.begin();
        game.command(0x62);
        game.write(STATUS_24H);
        game.end();
        assert_eq!(game.transfer_read(0x63, 1), [STATUS_24H]);
        assert_eq!(game.transfer_read(0x67, 1), [0x21]);
    }

    #[test]
    fn the_rom_shows_through_until_the_port_is_made_readable() {
        let mut gpio = Gpio::default();
        assert_eq!(gpio.read16(DATA), None);
        gpio.write16(CONTROL, 1);
        assert_eq!(gpio.read16(CONTROL), Some(1));
        gpio.write16(DIRECTION, 0b0101);
        assert_eq!(gpio.read16(DIRECTION), Some(0b0101));
        // Only the pins the game drives take its writes.
        gpio.write16(DATA, 0b1111);
        assert_eq!(gpio.read16(DATA), Some(0b0101));
        assert!(Gpio::covers(0xC4) && Gpio::covers(0xC9) && !Gpio::covers(0xCA));
    }

    #[test]
    fn a_byte_for_another_chip_is_ignored() {
        let mut gpio = Gpio::default();
        gpio.set_clock(afternoon());
        let bytes = Game(&mut gpio).transfer_read(0x15, 2);
        assert_eq!(bytes, [0, 0]);
    }
}
