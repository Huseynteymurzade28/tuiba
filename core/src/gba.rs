//! The whole system: CPU, bus and PPU wired together.

use crate::bios::{self, Outcome};
use crate::cpu::Cpu;
use crate::error::Result;
use crate::memory::dma::{self, Timing};
use crate::memory::io::reg;
use crate::memory::timers::Timers;
use crate::memory::{Bus, Cartridge, Memory};
use crate::ppu::{CYCLES_PER_LINE, Framebuffer, Ppu};

/// Cycles the system skips at a time while the CPU is halted. Small
/// enough that HBlank/VBlank events are not noticeably delayed.
const HALT_STEP: u32 = 32;

/// State of a pending `IntrWait`/`VBlankIntrWait` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IntrWait {
    mask: u16,
    resume_pc: u32,
}

/// A Game Boy Advance.
#[derive(Debug, Clone)]
pub struct Gba {
    /// The ARM7TDMI.
    pub cpu: Cpu,
    /// Memory and memory-mapped devices.
    pub bus: Bus,
    /// The LCD controller.
    pub ppu: Ppu,
    /// An in-progress `IntrWait` BIOS call: the interrupt mask it waits for
    /// and the address the call returns to.
    ///
    /// The real BIOS loops "halt; run the game's IRQ handler; check the
    /// flag word" until a matching flag appears. We mirror that: whenever
    /// the CPU is about to execute the return address outside IRQ mode, the
    /// flag word is checked and the CPU is either released or halted again.
    intr_wait: Option<IntrWait>,
    /// Number of the last BIOS call that could not be serviced, for the
    /// frontend to report.
    pub last_unsupported_swi: Option<u8>,
}

impl Gba {
    /// Boots a cartridge without a BIOS image: the CPU starts at the
    /// cartridge entry point and BIOS calls are emulated in software.
    #[must_use]
    pub fn new(cartridge: Cartridge) -> Self {
        let mut bus = Bus::new(cartridge);
        for (address, word) in bios::IRQ_STUB {
            bus.poke_bios(address, word);
        }
        let mut cpu = Cpu::new();
        cpu.hle_swi = true;
        cpu.skip_bios(&mut bus);
        // The BIOS leaves this flag set after the boot sequence.
        bus.io.write16(reg::POSTFLG, 1);
        Self {
            cpu,
            bus,
            ppu: Ppu::new(),
            intr_wait: None,
            last_unsupported_swi: None,
        }
    }

    /// Installs a real BIOS image and restarts from the reset vector.
    ///
    /// # Errors
    ///
    /// Returns [`crate::GbaError::BiosSize`] if the image is not 16 KiB.
    pub fn load_bios(&mut self, bios: &[u8]) -> Result<()> {
        self.bus.load_bios(bios)?;
        self.cpu.hle_swi = false;
        self.cpu.reset(&mut self.bus);
        self.intr_wait = None;
        Ok(())
    }

    /// The backup memory contents, for writing a save file.
    #[must_use]
    pub fn save_data(&self) -> &[u8] {
        self.bus.backup.data()
    }

    /// Restores backup memory from a save file's contents.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.bus.backup.load(data);
    }

    /// Sets the keypad state (`KEYINPUT` layout, active-low).
    pub fn set_keyinput(&mut self, keyinput: u16) {
        self.bus.io.keyinput = keyinput;
    }

    /// The most recently rendered frame.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.ppu.framebuffer
    }

    /// Runs the system until the PPU finishes a frame (enters VBlank).
    pub fn run_frame(&mut self) {
        loop {
            if self.step() {
                return;
            }
        }
    }

    /// Executes one instruction (or skips ahead while halted), advances the
    /// PPU, and services BIOS calls and interrupts. Returns `true` when a
    /// frame was completed.
    pub fn step(&mut self) -> bool {
        self.check_intr_wait();
        let cycles = if self.cpu.halted {
            HALT_STEP.min(CYCLES_PER_LINE)
        } else {
            let cycles = self.cpu.step(&mut self.bus);
            if let Some(number) = self.cpu.take_swi() {
                self.service_swi(number);
            }
            if std::mem::take(&mut self.bus.io.halt_requested) {
                self.cpu.halted = true;
            }
            cycles
        };

        self.run_dma();
        let timer_irqs = self.bus.io.timers.step(cycles);
        for n in (0..4).filter(|n| timer_irqs & (1 << n) != 0) {
            self.bus.io.request_interrupt(Timers::interrupt(n));
        }

        let events = self.ppu.step(cycles, &mut self.bus.io, &self.bus.video);
        if events.hblank {
            self.bus.io.dma.trigger(Timing::HBlank);
        }
        if events.vblank {
            self.bus.io.dma.trigger(Timing::VBlank);
        }
        self.run_dma();

        self.service_interrupts();
        events.vblank
    }

    /// Runs every DMA channel that has been triggered. Transfers are
    /// instantaneous; the CPU is simply not stepped in the meantime.
    fn run_dma(&mut self) {
        let pending = self.bus.io.dma.take_pending();
        for n in (0..4).filter(|n| pending & (1 << n) != 0) {
            // The transfer needs the whole bus, so the controller state is
            // moved out for its duration.
            let mut controller = std::mem::take(&mut self.bus.io.dma);
            self.hint_eeprom(&controller.channels[n], n);
            let irq = dma::run(&mut controller, n, &mut self.bus);
            self.bus.io.dma = controller;
            if let Some(irq) = irq {
                self.bus.io.request_interrupt(irq);
            }
        }
    }

    /// EEPROM chips cannot tell their own size; the length of the DMA that
    /// carries a request can (see [`memory::eeprom`]).
    fn hint_eeprom(&mut self, channel: &dma::Channel, n: usize) {
        if !channel.enabled() {
            return;
        }
        let (src, dst) = channel.latched_addresses();
        let units = channel.unit_count(n);
        self.bus.hint_eeprom_transfer(src, units);
        self.bus.hint_eeprom_transfer(dst, units);
    }

    fn service_swi(&mut self, number: u8) {
        match bios::service(number, &mut self.cpu, &mut self.bus) {
            Outcome::Done => {}
            Outcome::WaitForInterrupt { mask } => {
                self.intr_wait = Some(IntrWait {
                    mask,
                    resume_pc: self.cpu.next_pc(),
                });
            }
            Outcome::Unsupported => self.last_unsupported_swi = Some(number),
        }
    }

    /// Re-evaluates a pending `IntrWait` once the IRQ handler has returned
    /// to the call site: release the CPU if the game's handler flagged one
    /// of the awaited interrupts, otherwise halt again.
    fn check_intr_wait(&mut self) {
        let Some(wait) = self.intr_wait else { return };
        if self.cpu.halted
            || self.cpu.regs.mode() == crate::cpu::Mode::Irq
            || self.cpu.next_pc() != wait.resume_pc
        {
            return;
        }
        let flags = self.bus.read16(bios::INTR_CHECK_FLAGS);
        if flags & wait.mask != 0 {
            self.bus.write16(bios::INTR_CHECK_FLAGS, flags & !wait.mask);
            self.intr_wait = None;
        } else {
            self.cpu.halted = true;
        }
    }

    /// Wakes a halted CPU when an enabled interrupt is pending and enters
    /// the IRQ handler if the CPU accepts interrupts.
    fn service_interrupts(&mut self) {
        let io = &self.bus.io;
        let enabled_pending = io.read16(reg::IE) & io.read16(reg::IF);
        if enabled_pending == 0 {
            return;
        }
        // Any enabled interrupt wakes a halted CPU, even with IME off.
        self.cpu.halted = false;
        if io.read16(reg::IME) & 1 != 0 && self.cpu.irq_enabled() {
            self.cpu.raise_irq(&self.bus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::base;
    use crate::memory::io::Interrupt;
    use crate::memory::test_util::rom_with_header;
    use crate::ppu::{CYCLES_PER_FRAME, SCREEN_WIDTH};

    /// A cartridge whose entry point runs `code` (ARM).
    fn gba_with(code: &[u32]) -> Gba {
        let mut rom = rom_with_header("TEST", 0x1000);
        for (i, word) in code.iter().enumerate() {
            rom[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Gba::new(Cartridge::from_bytes(rom).unwrap())
    }

    #[test]
    fn boots_at_cartridge_entry_and_runs_a_frame() {
        // mov r0, #0x04000000 ; mov r1, #0x400 ; orr r1, r1, #3 ; strh r1, [r0] ; b .
        let mut gba = gba_with(&[
            0xE3A0_0301,
            0xE3A0_1B01,
            0xE381_1003,
            0xE1C0_10B0,
            0xEAFF_FFFE,
        ]);
        assert_eq!(gba.cpu.next_pc(), base::ROM_WS0);
        gba.run_frame();
        assert_eq!(gba.bus.io.read16(reg::DISPCNT), 0x0403);
        // A frame ends when VBlank starts, i.e. after the 160 visible lines.
        assert!(gba.cpu.cycles >= u64::from(160 * CYCLES_PER_LINE - CYCLES_PER_LINE));
        assert!(gba.cpu.cycles < u64::from(CYCLES_PER_FRAME));
        assert_eq!(gba.ppu.vcount(), 160);
    }

    #[test]
    fn mode3_pixel_reaches_framebuffer() {
        // mov r0, #0x04000000 ; mov r1, #0x400 ; orr r1, r1, #3 ; strh r1, [r0]
        // mov r2, #0x06000000 ; mov r3, #0x1F ; strh r3, [r2, #4] ; b .
        let mut gba = gba_with(&[
            0xE3A0_0301,
            0xE3A0_1B01,
            0xE381_1003,
            0xE1C0_10B0,
            0xE3A0_2406,
            0xE3A0_301F,
            0xE1C2_30B4,
            0xEAFF_FFFE,
        ]);
        gba.run_frame();
        assert_eq!(gba.framebuffer().row(0)[2], 0xFF00_00FF);
        assert_eq!(gba.framebuffer().pixels().len(), SCREEN_WIDTH * 160);
    }

    #[test]
    fn vblank_irq_is_delivered_and_intr_wait_resumes() {
        let mut gba = gba_with(&[
            0xE3A0_0301, // mov r0, #0x04000000
            0xE3A0_1008, // mov r1, #8            ; DISPSTAT: VBlank IRQ enable
            0xE1C0_10B4, // strh r1, [r0, #4]
            0xE280_4C02, // add r4, r0, #0x200
            0xE3A0_1001, // mov r1, #1
            0xE1C4_10B0, // strh r1, [r4]         ; IE = VBlank
            0xE1C4_10B8, // strh r1, [r4, #8]     ; IME = 1
            0xE3A0_2403, // mov r2, #0x03000000
            0xE382_2C7F, // orr r2, r2, #0x7F00
            0xE59F_100C, // ldr r1, =handler      ; literal at 0x38
            0xE582_10FC, // str r1, [r2, #0xFC]   ; 0x03007FFC = handler
            0xEF05_0000, // loop: swi VBlankIntrWait
            0xE3A0_3001, // mov r3, #1            ; marks a wakeup
            0xEAFF_FFFC, // b loop
            0x0800_0044, // =handler
            0,
            0,
            // handler @ 0x44: the game's IRQ routine, entered by the stub.
            0xE3A0_0301, // mov r0, #0x04000000
            0xE280_0C02, // add r0, r0, #0x200
            0xE3A0_1001, // mov r1, #1
            0xE1C0_10B2, // strh r1, [r0, #2]     ; IF = VBlank (ack)
            0xE3A0_2403, // mov r2, #0x03000000
            0xE382_2C7F, // orr r2, r2, #0x7F00
            0xE1C2_1FB8, // strh r1, [r2, #0xF8]  ; INTR_CHECK_FLAGS |= VBlank
            0xE12F_FF1E, // bx lr                 ; back into the stub
        ]);

        gba.run_frame();
        // The frame ends as VBlank fires, so the CPU is already in the stub.
        assert!(gba.intr_wait.is_some(), "still inside VBlankIntrWait");
        assert_eq!(gba.cpu.regs.mode(), crate::cpu::Mode::Irq);
        assert_eq!(gba.cpu.regs.get(3), 0);

        // Let the stub and handler run and return: the wait completes once.
        for _ in 0..64 {
            gba.step();
        }
        assert_eq!(gba.cpu.regs.get(3), 1, "woken once");
        assert_eq!(
            gba.bus.io.read16(reg::IF) & Interrupt::VBlank.mask(),
            0,
            "acknowledged"
        );
        assert_eq!(
            gba.cpu.regs.mode(),
            crate::cpu::Mode::System,
            "stub restored the mode"
        );
        assert!(gba.cpu.irq_enabled(), "handler returned and restored CPSR");
        assert!(gba.cpu.halted, "back in VBlankIntrWait");
        assert_eq!(gba.bus.read16(bios::INTR_CHECK_FLAGS), 0, "flag consumed");
        assert_eq!(gba.cpu.regs.get(13), 0x0300_7F00, "user stack balanced");
    }

    #[test]
    fn halt_wakes_on_enabled_interrupt() {
        // Enable the HBlank IRQ in IE but leave IME off; halt via HALTCNT.
        let mut gba = gba_with(&[
            0xE3A0_0301, // mov r0, #0x04000000
            0xE3A0_1010, // mov r1, #0x10         ; DISPSTAT: HBlank IRQ enable
            0xE1C0_10B4, // strh r1, [r0, #4]
            0xE280_4C02, // add r4, r0, #0x200
            0xE3A0_1002, // mov r1, #2
            0xE1C4_10B0, // strh r1, [r4]         ; IE = HBlank
            0xE3A0_1000, // mov r1, #0
            0xE5C0_1301, // strb r1, [r0, #0x301] ; HALTCNT
            0xE3A0_3001, // mov r3, #1
            0xEAFF_FFFE, // b .
        ]);
        for _ in 0..8 {
            gba.step();
        }
        assert!(gba.cpu.halted);
        assert_eq!(gba.cpu.regs.get(3), 0);
        while gba.cpu.halted {
            gba.step();
        }
        assert!(gba.cpu.cycles < 2000, "woke at the first HBlank");
        gba.step();
        assert_eq!(gba.cpu.regs.get(3), 1, "resumed after the halt");
    }

    #[test]
    fn immediate_dma_copies_rom_to_vram() {
        // DMA3: copy 4 halfwords from ROM+0x100 to VRAM.
        let mut gba = gba_with(&[
            0xE3A0_0301, // mov r0, #0x04000000
            0xE280_00D4, // add r0, r0, #0xD4    ; DMA3SAD
            0xE59F_1014, // ldr r1, =0x08000100
            0xE580_1000, // str r1, [r0]
            0xE3A0_1406, // mov r1, #0x06000000
            0xE580_1004, // str r1, [r0, #4]     ; DMA3DAD
            0xE59F_1008, // ldr r1, =0x80000004  ; enable, 4 halfwords
            0xE580_1008, // str r1, [r0, #8]     ; DMA3CNT
            0xEAFF_FFFE, // b .
            0x0800_0100,
            0x8000_0004,
        ]);
        gba.bus.cartridge = {
            let mut rom = gba.bus.cartridge.rom().to_vec();
            rom[0x100..0x108].copy_from_slice(&[1, 0, 2, 0, 3, 0, 4, 0]);
            Cartridge::from_bytes(rom).unwrap()
        };
        for _ in 0..10 {
            gba.step();
        }
        assert_eq!(gba.bus.read16(base::VRAM), 1);
        assert_eq!(gba.bus.read16(base::VRAM + 6), 4);
        assert_eq!(gba.bus.io.read16(reg::DMA3CNT_H) & 0x8000, 0, "done");
    }

    #[test]
    fn eeprom_is_driven_by_dma3() {
        let mut rom = rom_with_header("TEST", 0x1000);
        rom[..4].copy_from_slice(&0xEAFF_FFFE_u32.to_le_bytes()); // b .
        rom[0x200..0x209].copy_from_slice(b"EEPROM_V1");
        let mut gba = Gba::new(Cartridge::from_bytes(rom).unwrap());
        let dma3 = |gba: &mut Gba, src: u32, dst: u32, units: u32| {
            gba.bus.io.write32(reg::DMA0SAD + 12 * 3, src);
            gba.bus.io.write32(reg::DMA0SAD + 12 * 3 + 4, dst);
            gba.bus
                .io
                .write32(reg::DMA0SAD + 12 * 3 + 8, 0x8000_0000 | units);
            gba.step();
        };
        let buffer = base::EWRAM;
        let put_bits = |gba: &mut Gba, bits: &[u16]| {
            for (i, &bit) in bits.iter().enumerate() {
                gba.bus.write16(buffer + i as u32 * 2, bit);
            }
        };

        // Write request for a 512 B chip: `10`, 6-bit address 5, 64 data
        // bits (0x80...01), stop bit — 73 halfwords, which fixes the size.
        let mut request = vec![1, 0, 0, 0, 0, 1, 0, 1];
        request.extend((0..64).map(|i| u16::from(i == 0 || i == 63)));
        request.push(0);
        put_bits(&mut gba, &request);
        dma3(&mut gba, buffer, 0x0D00_0000, 73);
        assert_eq!(gba.save_data().len(), 0x200, "size learnt from the DMA");
        assert_eq!(&gba.save_data()[5 * 8..6 * 8], &[0x80, 0, 0, 0, 0, 0, 0, 1]);

        // Read request: `11`, address 5, stop bit — 9 halfwords — then fetch
        // the 68-bit reply.
        put_bits(&mut gba, &[1, 1, 0, 0, 0, 1, 0, 1, 0]);
        dma3(&mut gba, buffer, 0x0D00_0000, 9);
        dma3(&mut gba, 0x0D00_0000, buffer, 68);
        let reply: Vec<u16> = (0..68)
            .map(|i| gba.bus.read16(buffer + i * 2) & 1)
            .collect();
        let mut expected = vec![0; 4];
        expected.extend((0..64).map(|i| u16::from(i == 0 || i == 63)));
        assert_eq!(reply, expected);
        assert_eq!(gba.bus.read16(0x0D00_0000), 1, "ready again");

        // Loading a save restores the chip.
        let mut other = Gba::new(Cartridge::from_bytes(gba.bus.cartridge.rom().to_vec()).unwrap());
        other.load_save_data(gba.save_data());
        assert_eq!(other.save_data(), gba.save_data());
    }

    #[test]
    fn timer_overflow_raises_interrupt() {
        let mut gba = gba_with(&[0xEAFF_FFFE]); // b .
        gba.bus.io.write16(reg::TM0CNT_L, 0xFFF0);
        gba.bus.io.write16(reg::TM0CNT_H, 0x80 | 0x40);
        gba.bus.io.write16(reg::IE, 1 << 3);
        for _ in 0..8 {
            gba.step();
        }
        assert_ne!(gba.bus.io.read16(reg::IF) & (1 << 3), 0);
    }

    #[test]
    fn unsupported_swi_is_reported() {
        let mut gba = gba_with(&[0xEF2A_0000, 0xEAFF_FFFE]);
        gba.step();
        assert_eq!(gba.last_unsupported_swi, Some(0x2A));
    }
}
