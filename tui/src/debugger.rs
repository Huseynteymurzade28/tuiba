//! The debugger panel: the CPU's registers and the code around the PC
//! of a paused game, and stepping it an instruction, a scanline or a
//! frame at a time.
//!
//! The panel sits beside the game screen when the terminal is wide
//! enough, so a step shows what it drew; on a narrow one it takes the
//! whole screen. Emulation waits while it is up, like it does for the
//! other panels: only the steps asked for here run the machine.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use tuiba_core::cpu::disasm;
use tuiba_core::cpu::{Cpsr, Mode};
use tuiba_core::{Gba, Memory};

use crate::gamepad::{PadButton, PadStyle};
use crate::theme;

/// Panel width in cells: room for a THUMB `bl` with its operand.
pub const WIDTH: u16 = 60;

/// The narrowest game screen worth keeping beside the panel.
const MIN_SCREEN_WIDTH: u16 = 40;

/// How far one step runs the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// One instruction; a halted CPU runs until it executes one.
    Instruction,
    /// Until the PPU moves to the next scanline.
    Line,
    /// Until the PPU finishes the frame.
    Frame,
}

/// What the panel wants done after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Step(Step),
    Close,
}

/// The debugger over a paused game. It has no state of its own yet:
/// everything it shows is read from the machine on each draw.
pub struct DebuggerPanel;

// Methods rather than functions: the memory view and breakpoints will
// keep their state here.
#[allow(clippy::unused_self)]
impl DebuggerPanel {
    #[must_use]
    pub const fn open() -> Self {
        Self
    }

    /// Turns the panel's keys into an [`Action`]. Everything else is
    /// swallowed: the game must not see the keyboard while this is up.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        match key.code {
            KeyCode::Char('i' | 's') | KeyCode::Enter => Some(Action::Step(Step::Instruction)),
            KeyCode::Char('l') => Some(Action::Step(Step::Line)),
            KeyCode::Char('f' | '.') => Some(Action::Step(Step::Frame)),
            KeyCode::Esc => Some(Action::Close),
            _ => None,
        }
    }

    /// The key a pad button stands for in the panel: the pad's confirm
    /// button steps an instruction, the shoulders a scanline and a
    /// frame, and its back button closes.
    #[must_use]
    pub fn pad_key(button: PadButton, style: PadStyle) -> Option<KeyCode> {
        Some(match button {
            PadButton::R1 => KeyCode::Char('l'),
            PadButton::L1 => KeyCode::Char('f'),
            b if b == style.confirm() => KeyCode::Enter,
            b if b == style.back() => KeyCode::Esc,
            _ => return None,
        })
    }

    /// Splits `area` into the game screen and the panel. The screen is
    /// empty when there is no room for both.
    #[must_use]
    pub fn split(area: Rect) -> (Rect, Rect) {
        if area.width >= WIDTH + MIN_SCREEN_WIDTH {
            let [screen, panel] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(WIDTH)]).areas(area);
            (screen, panel)
        } else {
            (Rect { width: 0, ..area }, area)
        }
    }

    /// Draws the panel into `area`. With `pad`, the footer names that
    /// pad's buttons instead of keys.
    pub fn draw(&self, frame: &mut Frame, area: Rect, gba: &Gba, pad: Option<PadStyle>) {
        frame.render_widget(Clear, area);
        let cpsr = gba.cpu.regs.cpsr;
        let state = if cpsr.thumb() { "THUMB" } else { "ARM" };
        let title = format!(" DEBUGGER · {state} · {} ", mode_name(cpsr.mode()));
        let block = Block::default()
            .title(Line::styled(title, theme::accent().bold()))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border())
            .padding(Padding::horizontal(1))
            .style(theme::text().bg(theme::SURFACE));
        let inside = block.inner(area);
        frame.render_widget(block, area);
        let [registers, status, _, code, footer] = Layout::vertical([
            Constraint::Length(8),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inside);

        frame.render_widget(Paragraph::new(register_lines(gba)), registers);
        frame.render_widget(Paragraph::new(status_lines(gba)), status);
        let rows = usize::from(code.height);
        frame.render_widget(Paragraph::new(code_lines(gba, rows)), code);
        frame.render_widget(footer_line(pad), footer);
    }
}

/// Runs `gba` by `step`, without input, sound or cheats: the machine
/// alone. Returns whether a frame was finished on the way.
pub fn run(gba: &mut Gba, step: Step) -> bool {
    let mut finished = false;
    match step {
        Step::Instruction => loop {
            let halted = gba.cpu.halted;
            finished |= gba.step();
            if !halted || finished {
                break;
            }
        },
        Step::Line => {
            let line = gba.ppu.vcount();
            while gba.ppu.vcount() == line {
                finished |= gba.step();
            }
        }
        Step::Frame => {
            while !gba.step() {}
            finished = true;
        }
    }
    gba.clear_audio();
    finished
}

fn mode_name(mode: Option<Mode>) -> &'static str {
    match mode {
        Some(Mode::User) => "user",
        Some(Mode::Fiq) => "fiq",
        Some(Mode::Irq) => "irq",
        Some(Mode::Supervisor) => "svc",
        Some(Mode::Abort) => "abort",
        Some(Mode::Undefined) => "undef",
        Some(Mode::System) => "system",
        None => "invalid",
    }
}

const REGISTER_NAMES: [&str; 16] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "sp", "lr",
    "pc",
];

/// r0–r7 on the left, r8–pc on the right. The PC is the address of the
/// next instruction, not r15 with the pipeline's lead.
fn register_lines(gba: &Gba) -> Vec<Line<'static>> {
    let value = |n: usize| {
        if n == 15 {
            gba.cpu.next_pc()
        } else {
            gba.cpu.regs.get(n)
        }
    };
    (0..8)
        .map(|row| {
            let mut spans = Vec::new();
            for n in [row, row + 8] {
                spans.push(Span::styled(
                    format!("{:<4}", REGISTER_NAMES[n]),
                    theme::hint(),
                ));
                spans.push(Span::styled(format!("{:08x}", value(n)), theme::text()));
                spans.push(Span::raw("      "));
            }
            Line::from(spans)
        })
        .collect()
}

/// The flags, the saved PSR of the current mode, and where the PPU is.
fn status_lines(gba: &Gba) -> Vec<Line<'static>> {
    let cpsr = gba.cpu.regs.cpsr;
    let mut psr = vec![
        Span::styled("cpsr ", theme::hint()),
        Span::styled(format!("{:08x}  ", cpsr.0), theme::text()),
    ];
    psr.extend(flag_spans(cpsr));
    if let Some(spsr) = gba.cpu.regs.spsr() {
        psr.push(Span::styled("  spsr ", theme::hint()));
        psr.push(Span::styled(format!("{:08x}", spsr.0), theme::text()));
    }
    let mut timing = vec![
        Span::styled("line ", theme::hint()),
        Span::styled(format!("{:>4}", gba.ppu.vcount()), theme::text()),
        Span::styled("  cycle", theme::hint()),
        Span::styled(format!("{:>5}", gba.ppu.line_cycle()), theme::text()),
        Span::styled("  total", theme::hint()),
        Span::styled(format!(" {}", gba.cpu.cycles), theme::text()),
    ];
    if gba.cpu.halted {
        timing.push(Span::styled("  halted", Style::default().fg(theme::WARN)));
    }
    vec![Line::from(psr), Line::from(timing)]
}

/// `NZCV IFT`, upper case when set.
fn flag_spans(cpsr: Cpsr) -> Vec<Span<'static>> {
    let flags = [
        ('n', cpsr.n()),
        ('z', cpsr.z()),
        ('c', cpsr.c()),
        ('v', cpsr.v()),
        (' ', false),
        ('i', cpsr.irq_disabled()),
        ('f', cpsr.fiq_disabled()),
        ('t', cpsr.thumb()),
    ];
    flags
        .into_iter()
        .map(|(name, set)| {
            if set {
                Span::styled(
                    name.to_ascii_uppercase().to_string(),
                    theme::accent().bold(),
                )
            } else {
                Span::styled(name.to_string(), theme::hint())
            }
        })
        .collect()
}

/// `rows` lines of code with the next instruction a third of the way
/// down, so what led to it shows as well as what follows.
fn code_lines(gba: &Gba, rows: usize) -> Vec<Line<'static>> {
    let pc = gba.cpu.next_pc();
    let thumb = gba.cpu.thumb();
    let size = if thumb { 2 } else { 4 };
    let before = u32::try_from(rows / 3).unwrap_or(0);
    let mut address = pc.wrapping_sub(before * size);
    let mut lines = Vec::with_capacity(rows);
    while lines.len() < rows {
        let (text, encoding, mut length) = if thumb {
            let half = gba.bus.read16(address);
            let next = gba.bus.read16(address.wrapping_add(2));
            let (text, length) = disasm::thumb(address, half, next);
            let encoding = if length == 4 {
                format!("{half:04x}{next:04x}")
            } else {
                format!("{half:04x}")
            };
            (text, encoding, length)
        } else {
            let word = gba.bus.read32(address);
            (disasm::arm(address, word), format!("{word:08x}"), 4)
        };
        // A `bl` pair read from its second half would swallow the PC:
        // never step past the instruction about to run.
        if address < pc && address.wrapping_add(length) > pc {
            length = pc - address;
        }
        let current = address == pc;
        let (marker, style) = if current {
            ("▶ ", theme::accent().bold())
        } else {
            ("  ", theme::text())
        };
        lines.push(Line::from(vec![
            Span::styled(marker, theme::accent()),
            Span::styled(format!("{address:08x}  "), theme::hint()),
            Span::styled(format!("{encoding:<10}"), theme::hint()),
            Span::styled(text, style),
        ]));
        address = address.wrapping_add(length);
    }
    lines
}

fn footer_line(pad: Option<PadStyle>) -> Paragraph<'static> {
    let style = pad.unwrap_or(PadStyle::Generic);
    let (step, close, line, frame) = if pad.is_some() {
        (
            style.label(style.confirm()),
            style.label(style.back()),
            style.label(PadButton::R1),
            style.label(PadButton::L1),
        )
    } else {
        ("i", "esc", "l", "f")
    };
    let mut spans = Vec::new();
    for (key, label) in [
        (step, "instruction"),
        (line, "line"),
        (frame, "frame"),
        (close, "close"),
    ] {
        spans.push(Span::styled(format!(" {key} "), theme::key()));
        spans.push(Span::styled(format!("{label}  "), theme::hint()));
    }
    Paragraph::new(Line::from(spans)).alignment(Alignment::Center)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuiba_core::Cartridge;

    /// `mov r0, #1; add r0, r0, #1; b .-4` at the entry point, ARM.
    fn gba() -> Gba {
        let mut rom = vec![0u8; 0xC0];
        let words: [u32; 3] = [0xE3A0_0001, 0xE280_0001, 0xEAFF_FFFD];
        for (i, word) in words.iter().enumerate() {
            rom[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Gba::new(Cartridge::from_bytes(rom).expect("valid ROM"))
    }

    #[test]
    fn an_instruction_step_runs_one_instruction() {
        let mut gba = gba();
        assert_eq!(gba.cpu.next_pc(), 0x0800_0000);
        run(&mut gba, Step::Instruction);
        assert_eq!(gba.cpu.next_pc(), 0x0800_0004);
        assert_eq!(gba.cpu.regs.get(0), 1);
        run(&mut gba, Step::Instruction);
        assert_eq!(gba.cpu.regs.get(0), 2);
    }

    #[test]
    fn a_line_step_moves_to_the_next_scanline() {
        let mut gba = gba();
        let line = gba.ppu.vcount();
        run(&mut gba, Step::Line);
        assert_eq!(gba.ppu.vcount(), (line + 1) % 228);
    }

    #[test]
    fn a_frame_step_ends_at_vblank() {
        let mut gba = gba();
        assert!(run(&mut gba, Step::Frame));
        assert_eq!(gba.ppu.vcount(), 160);
    }

    #[test]
    fn the_code_view_puts_the_pc_a_third_down() {
        let mut gba = gba();
        run(&mut gba, Step::Instruction);
        let lines = code_lines(&gba, 6);
        assert_eq!(lines.len(), 6);
        let text = |n: usize| {
            lines[n]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        assert!(text(2).starts_with("▶ 08000004"), "{}", text(2));
        assert!(text(2).contains("add r0, r0, #1"), "{}", text(2));
        assert!(text(1).contains("mov r0, #1"), "{}", text(1));
    }

    #[test]
    fn narrow_terminals_give_the_panel_everything() {
        let wide = Rect::new(0, 0, 120, 40);
        let (screen, panel) = DebuggerPanel::split(wide);
        assert_eq!(panel.width, WIDTH);
        assert_eq!(screen.width, 120 - WIDTH);
        let narrow = Rect::new(0, 0, 80, 24);
        let (screen, panel) = DebuggerPanel::split(narrow);
        assert_eq!(screen.width, 0);
        assert_eq!(panel, narrow);
    }
}
