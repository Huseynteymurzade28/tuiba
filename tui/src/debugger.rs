//! The debugger panel: the CPU's registers and the code around the PC
//! of a paused game, and stepping it an instruction, a scanline or a
//! frame at a time.
//!
//! The panel sits beside the game screen when the terminal is wide
//! enough, so a step shows what it drew; on a narrow one it takes the
//! whole screen. Emulation waits while it is up, like it does for the
//! other panels: only the steps asked for here run the machine.
//!
//! Breakpoints outlive the panel: closing it lets the game run on until
//! the PC reaches one, which opens the panel again.

use std::collections::BTreeSet;

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

/// Addresses where running code stops before executing them.
pub type Breakpoints = BTreeSet<u32>;

/// What the panel wants done after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Step(Step),
    /// Set a breakpoint here, or clear the one that is.
    Toggle(u32),
    Close,
}

/// What `g` or `b` is typing an address for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prompt {
    GoTo,
    Break,
}

/// Which view fills the lower half of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Code,
    Memory,
}

/// Bytes per row of the memory view.
const MEMORY_ROW: u32 = 8;

/// Where IWRAM starts: the memory view opens there, on the game's
/// variables and stacks.
const IWRAM: u32 = 0x0300_0000;

/// The debugger over a paused game. It keeps only where its views are
/// looking; everything it shows is read from the machine on each draw.
pub struct DebuggerPanel {
    view: View,
    /// First address of the code view; `None` follows the PC.
    code_at: Option<u32>,
    /// First address of the memory view.
    memory_at: u32,
    /// The address being typed after `g` or `b`.
    prompt: Option<(Prompt, String)>,
    /// Why the last address typed was refused.
    error: Option<&'static str>,
    /// Rows the lower view had at the last draw, for paging.
    rows: u32,
    /// The memory view's bytes before the last step, to mark what the
    /// step changed, and the address they start at.
    before: Vec<u8>,
    before_at: u32,
}

impl DebuggerPanel {
    #[must_use]
    pub const fn open() -> Self {
        Self {
            view: View::Code,
            code_at: None,
            memory_at: IWRAM,
            prompt: None,
            error: None,
            rows: 16,
            before: Vec::new(),
            before_at: 0,
        }
    }

    /// Turns the panel's keys into an [`Action`], and moves its views.
    /// Everything else is swallowed: the game must not see the keyboard
    /// while this is up. `gba` resolves register names typed after `g`
    /// or `b`.
    pub fn handle(&mut self, key: KeyEvent, gba: &Gba) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        self.error = None;
        if let Some((_, text)) = &mut self.prompt {
            match key.code {
                KeyCode::Char(c) if c.is_ascii_alphanumeric() && text.len() < 10 => {
                    text.push(c.to_ascii_lowercase());
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Enter => {
                    let (prompt, text) = self.prompt.take()?;
                    // An empty `b` means the instruction about to run.
                    let at = if prompt == Prompt::Break && text.is_empty() {
                        Some(gba.cpu.next_pc())
                    } else {
                        address(&text, gba)
                    };
                    match (prompt, at) {
                        (Prompt::GoTo, Some(at)) => self.go_to(at, gba),
                        (Prompt::Break, Some(at)) => {
                            return Some(Action::Toggle(at & !(instruction_size(gba) - 1)));
                        }
                        (_, None) => self.error = Some("not an address or register"),
                    }
                }
                KeyCode::Esc => self.prompt = None,
                _ => {}
            }
            return None;
        }
        let page = self.rows.max(1);
        match key.code {
            KeyCode::Char('i' | 's') | KeyCode::Enter => {
                return Some(self.step(Step::Instruction, gba));
            }
            KeyCode::Char('l') => return Some(self.step(Step::Line, gba)),
            KeyCode::Char('f' | '.') => return Some(self.step(Step::Frame, gba)),
            KeyCode::Esc => return Some(Action::Close),
            KeyCode::Tab => {
                self.view = match self.view {
                    View::Code => View::Memory,
                    View::Memory => View::Code,
                };
            }
            KeyCode::Char('g') => self.prompt = Some((Prompt::GoTo, String::new())),
            KeyCode::Char('b') => self.prompt = Some((Prompt::Break, String::new())),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1, gba),
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1, gba),
            KeyCode::PageUp => self.scroll(-i64::from(page), gba),
            KeyCode::PageDown => self.scroll(i64::from(page), gba),
            KeyCode::Home if self.view == View::Code => self.code_at = None,
            _ => {}
        }
        None
    }

    /// A step, with the memory view's bytes kept from before it.
    fn step(&mut self, step: Step, gba: &Gba) -> Action {
        let length = self.rows.max(1) * MEMORY_ROW;
        self.before = (0..length)
            .map(|n| gba.bus.read8(self.memory_at.wrapping_add(n)))
            .collect();
        self.before_at = self.memory_at;
        // A step moves the PC: the code view goes back to following it.
        self.code_at = None;
        Action::Step(step)
    }

    /// Points the current view at `at`.
    fn go_to(&mut self, at: u32, gba: &Gba) {
        match self.view {
            View::Code => self.code_at = Some(at & !(instruction_size(gba) - 1)),
            View::Memory => self.memory_at = at & !(MEMORY_ROW - 1),
        }
    }

    /// Moves the current view by `rows` rows.
    fn scroll(&mut self, rows: i64, gba: &Gba) {
        let (at, row) = match self.view {
            View::Code => (
                self.code_at.get_or_insert(code_start(gba, self.rows)),
                instruction_size(gba),
            ),
            View::Memory => (&mut self.memory_at, MEMORY_ROW),
        };
        // Two's complement: a negative offset wraps the right way.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let offset = (rows * i64::from(row)) as u32;
        *at = at.wrapping_add(offset);
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

    /// Draws the panel into `area`, the code view marking `breakpoints`.
    /// With `pad`, the footer names that pad's buttons instead of keys.
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        gba: &Gba,
        breakpoints: &Breakpoints,
        pad: Option<PadStyle>,
    ) {
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
        let [registers, status, heading, lower, footer] = Layout::vertical([
            Constraint::Length(8),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(inside);

        frame.render_widget(Paragraph::new(register_lines(gba)), registers);
        frame.render_widget(Paragraph::new(status_lines(gba)), status);
        self.rows = u32::from(lower.height);
        let rows = usize::from(lower.height);
        let (heading_text, lines) = match self.view {
            View::Code => {
                let start = self.code_at.unwrap_or_else(|| code_start(gba, self.rows));
                let place = if self.code_at.is_some() {
                    format!("at {start:08x}  (home: back to pc)")
                } else {
                    "following pc".to_owned()
                };
                let marks = match breakpoints.len() {
                    0 => String::new(),
                    1 => " · ● 1 breakpoint".to_owned(),
                    n => format!(" · ● {n} breakpoints"),
                };
                (
                    format!("code · {place}{marks}"),
                    code_lines(gba, start, rows, breakpoints),
                )
            }
            View::Memory => {
                let before = (self.before_at == self.memory_at).then_some(self.before.as_slice());
                (
                    format!("memory · {} {:08x}", region(self.memory_at), self.memory_at),
                    memory_lines(gba, self.memory_at, rows, before),
                )
            }
        };
        frame.render_widget(
            Paragraph::new(Line::styled(heading_text, theme::accent())),
            heading,
        );
        frame.render_widget(Paragraph::new(lines), lower);
        let footer_widget = match (&self.prompt, self.error) {
            (Some((prompt, text)), _) => prompt_line(*prompt, text, self.view),
            (None, Some(error)) => {
                Paragraph::new(Line::styled(error, Style::default().fg(theme::WARN)))
                    .alignment(Alignment::Center)
            }
            (None, None) => footer_line(pad, self.view),
        };
        frame.render_widget(footer_widget, footer);
    }
}

fn instruction_size(gba: &Gba) -> u32 {
    if gba.cpu.thumb() { 2 } else { 4 }
}

/// Where the code view starts when it follows the PC: a third of
/// `rows` above it, so what led to it shows as well as what follows.
fn code_start(gba: &Gba, rows: u32) -> u32 {
    gba.cpu
        .next_pc()
        .wrapping_sub(rows / 3 * instruction_size(gba))
}

/// What `g` or `b` was given: a register name, or a hexadecimal address with
/// or without `0x`.
fn address(text: &str, gba: &Gba) -> Option<u32> {
    if let Some(n) = REGISTER_NAMES.iter().position(|&name| name == text) {
        return Some(if n == 15 {
            gba.cpu.next_pc()
        } else {
            gba.cpu.regs.get(n)
        });
    }
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.is_empty() {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// The name of the memory area `address` is in.
fn region(address: u32) -> &'static str {
    match address >> 24 {
        0x00 => "BIOS",
        0x02 => "EWRAM",
        0x03 => "IWRAM",
        0x04 => "I/O",
        0x05 => "palette",
        0x06 => "VRAM",
        0x07 => "OAM",
        0x08..=0x0D => "ROM",
        0x0E | 0x0F => "SRAM",
        _ => "unmapped",
    }
}

/// `rows` rows of eight bytes from `start`: address, hex and the bytes
/// as text. Bytes that differ from `before` (what the same rows held
/// before the last step) stand out.
fn memory_lines(gba: &Gba, start: u32, rows: usize, before: Option<&[u8]>) -> Vec<Line<'static>> {
    (0..rows)
        .map(|row| {
            #[allow(clippy::cast_possible_truncation)] // a screen has few rows
            let at = start.wrapping_add(row as u32 * MEMORY_ROW);
            let mut spans = vec![Span::styled(format!("{at:08x}  "), theme::hint())];
            let mut text = String::with_capacity(MEMORY_ROW as usize);
            for n in 0..MEMORY_ROW {
                let byte = gba.bus.read8(at.wrapping_add(n));
                let index = row * MEMORY_ROW as usize + n as usize;
                let changed = before
                    .and_then(|b| b.get(index))
                    .is_some_and(|&old| old != byte);
                let style = if changed {
                    Style::default().fg(theme::WARN).bold()
                } else {
                    theme::text()
                };
                spans.push(Span::styled(format!("{byte:02x} "), style));
                text.push(if byte.is_ascii_graphic() || byte == b' ' {
                    char::from(byte)
                } else {
                    '.'
                });
            }
            spans.push(Span::styled(format!(" {text}"), theme::hint()));
            Line::from(spans)
        })
        .collect()
}

/// How a run ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ran {
    /// A frame was finished on the way.
    pub frame: bool,
    /// The PC reached a breakpoint, and the instruction there has not
    /// run.
    pub breakpoint: bool,
}

/// Runs `gba` by `step`, without input, sound or cheats: the machine
/// alone. The instruction at the PC always runs, breakpoint or not, so
/// a step from one moves on; after it, reaching a breakpoint ends the
/// run early.
pub fn run(gba: &mut Gba, step: Step, breakpoints: &Breakpoints) -> Ran {
    let ran = advance(gba, step, breakpoints);
    gba.clear_audio();
    ran
}

/// Runs `gba` to the end of the frame like [`Gba::run_frame`], but
/// stops early at a breakpoint. `resume` lets the instruction at the PC
/// run even if it has one: the game is carrying on from it.
pub fn run_frame(gba: &mut Gba, breakpoints: &Breakpoints, resume: bool) -> Ran {
    if resume {
        advance(gba, Step::Frame, breakpoints)
    } else if hit(gba, breakpoints) {
        Ran {
            frame: false,
            breakpoint: true,
        }
    } else {
        advance(gba, Step::Frame, breakpoints)
    }
}

/// Whether the instruction about to run has a breakpoint. A halted CPU
/// is about to run nothing.
fn hit(gba: &Gba, breakpoints: &Breakpoints) -> bool {
    !gba.cpu.halted && breakpoints.contains(&gba.cpu.next_pc())
}

fn advance(gba: &mut Gba, step: Step, breakpoints: &Breakpoints) -> Ran {
    let line = gba.ppu.vcount();
    let mut ran = Ran::default();
    loop {
        let halted = gba.cpu.halted;
        ran.frame |= gba.step();
        let done = match step {
            // A halted CPU runs until it executes an instruction.
            Step::Instruction => !halted || ran.frame,
            Step::Line => gba.ppu.vcount() != line,
            Step::Frame => ran.frame,
        };
        if done {
            return ran;
        }
        if hit(gba, breakpoints) {
            ran.breakpoint = true;
            return ran;
        }
    }
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

/// `rows` lines of code from `start`, the next instruction and the
/// breakpoints marked.
fn code_lines(gba: &Gba, start: u32, rows: usize, breakpoints: &Breakpoints) -> Vec<Line<'static>> {
    let pc = gba.cpu.next_pc();
    let thumb = gba.cpu.thumb();
    let mut address = start;
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
        let style = if current {
            theme::accent().bold()
        } else {
            theme::text()
        };
        let stop = if breakpoints.contains(&address) {
            "●"
        } else {
            " "
        };
        lines.push(Line::from(vec![
            Span::styled(stop, Style::default().fg(theme::WARN)),
            Span::styled(if current { "▶ " } else { "  " }, theme::accent()),
            Span::styled(format!("{address:08x}  "), theme::hint()),
            Span::styled(format!("{encoding:<10}"), theme::hint()),
            Span::styled(text, style),
        ]));
        address = address.wrapping_add(length);
    }
    lines
}

fn footer_line(pad: Option<PadStyle>, view: View) -> Paragraph<'static> {
    let other = match view {
        View::Code => "memory",
        View::Memory => "code",
    };
    let rows: Vec<Vec<(String, &str)>> = if let Some(style) = pad {
        vec![vec![
            (style.label(style.confirm()).to_owned(), "instr"),
            (style.label(PadButton::R1).to_owned(), "line"),
            (style.label(PadButton::L1).to_owned(), "frame"),
            (style.label(style.back()).to_owned(), "run"),
        ]]
    } else {
        vec![
            vec![
                ("i".to_owned(), "instruction"),
                ("l".to_owned(), "line"),
                ("f".to_owned(), "frame"),
                ("esc".to_owned(), "run"),
            ],
            vec![
                ("b".to_owned(), "breakpoint"),
                ("g".to_owned(), "go to"),
                ("tab".to_owned(), other),
            ],
        ]
    };
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|row| {
            let mut spans = Vec::new();
            for (key, label) in row {
                spans.push(Span::styled(format!(" {key} "), theme::key()));
                spans.push(Span::styled(format!("{label}  "), theme::hint()));
            }
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).alignment(Alignment::Center)
}

/// The `g` or `b` prompt: what has been typed, and what it accepts.
fn prompt_line(prompt: Prompt, text: &str, view: View) -> Paragraph<'static> {
    let (what, accepts) = match (prompt, view) {
        (Prompt::GoTo, View::Code) => ("code at", "hex or r0–pc"),
        (Prompt::GoTo, View::Memory) => ("memory at", "hex or r0–pc"),
        (Prompt::Break, _) => ("breakpoint at", "hex, r0–pc, or ⏎ for pc"),
    };
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!("{what} "), theme::hint()),
            Span::styled(format!("{text}▏"), theme::text()),
        ]),
        Line::from(vec![
            Span::styled(format!("{accepts}  "), theme::hint()),
            Span::styled(" ⏎ ", theme::key()),
            Span::styled(" esc ", theme::key()),
        ]),
    ])
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
        run(&mut gba, Step::Instruction, &Breakpoints::new());
        assert_eq!(gba.cpu.next_pc(), 0x0800_0004);
        assert_eq!(gba.cpu.regs.get(0), 1);
        run(&mut gba, Step::Instruction, &Breakpoints::new());
        assert_eq!(gba.cpu.regs.get(0), 2);
    }

    #[test]
    fn a_line_step_moves_to_the_next_scanline() {
        let mut gba = gba();
        let line = gba.ppu.vcount();
        run(&mut gba, Step::Line, &Breakpoints::new());
        assert_eq!(gba.ppu.vcount(), (line + 1) % 228);
    }

    #[test]
    fn a_frame_step_ends_at_vblank() {
        let mut gba = gba();
        assert!(run(&mut gba, Step::Frame, &Breakpoints::new()).frame);
        assert_eq!(gba.ppu.vcount(), 160);
    }

    #[test]
    fn the_code_view_puts_the_pc_a_third_down() {
        let mut gba = gba();
        run(&mut gba, Step::Instruction, &Breakpoints::new());
        let lines = code_lines(&gba, code_start(&gba, 6), 6, &Breakpoints::new());
        assert_eq!(lines.len(), 6);
        let text = |n: usize| {
            lines[n]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        assert!(text(2).starts_with(" ▶ 08000004"), "{}", text(2));
        assert!(text(2).contains("add r0, r0, #1"), "{}", text(2));
        assert!(text(1).contains("mov r0, #1"), "{}", text(1));
    }

    fn press(panel: &mut DebuggerPanel, gba: &Gba, code: KeyCode) -> Option<Action> {
        panel.handle(KeyEvent::from(code), gba)
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn go_to_takes_hex_or_a_register() {
        let mut gba = gba();
        run(&mut gba, Step::Instruction, &Breakpoints::new());
        assert_eq!(address("3000010", &gba), Some(0x0300_0010));
        assert_eq!(address("0x6000000", &gba), Some(0x0600_0000));
        assert_eq!(address("r0", &gba), Some(1));
        assert_eq!(address("pc", &gba), Some(0x0800_0004));
        assert_eq!(address("", &gba), None);
        assert_eq!(address("0x", &gba), None);
        assert_eq!(address("r16", &gba), None);
        assert_eq!(address("123456789", &gba), None);
    }

    #[test]
    fn the_prompt_moves_the_memory_view() {
        let gba = gba();
        let mut panel = DebuggerPanel::open();
        press(&mut panel, &gba, KeyCode::Tab);
        press(&mut panel, &gba, KeyCode::Char('g'));
        for c in "600000c".chars() {
            assert_eq!(
                press(&mut panel, &gba, KeyCode::Char(c)),
                None,
                "typing is not a step"
            );
        }
        press(&mut panel, &gba, KeyCode::Enter);
        assert_eq!(panel.memory_at, 0x0600_0008, "rounded down to a row");
        press(&mut panel, &gba, KeyCode::Char('g'));
        press(&mut panel, &gba, KeyCode::Char('x'));
        press(&mut panel, &gba, KeyCode::Enter);
        assert!(panel.error.is_some());
        assert_eq!(panel.memory_at, 0x0600_0008);
        press(&mut panel, &gba, KeyCode::Up);
        assert_eq!(panel.memory_at, 0x0600_0000);
    }

    #[test]
    fn scrolling_the_code_stops_following_until_a_step() {
        let gba = gba();
        let mut panel = DebuggerPanel::open();
        press(&mut panel, &gba, KeyCode::Down);
        assert_eq!(panel.code_at, Some(code_start(&gba, panel.rows) + 4));
        press(&mut panel, &gba, KeyCode::Home);
        assert_eq!(panel.code_at, None);
        press(&mut panel, &gba, KeyCode::Down);
        assert_eq!(
            press(&mut panel, &gba, KeyCode::Char('i')),
            Some(Action::Step(Step::Instruction))
        );
        assert_eq!(panel.code_at, None);
    }

    #[test]
    fn the_memory_view_marks_what_a_step_changed() {
        let mut gba = gba();
        gba.bus.write8(0x0300_0001, 0x41);
        let before = vec![0u8; 16];
        let lines = memory_lines(&gba, 0x0300_0000, 2, Some(&before));
        assert_eq!(
            text(&lines[0]),
            "03000000  00 41 00 00 00 00 00 00  .A......"
        );
        let marked: Vec<_> = lines[0].spans[1..9]
            .iter()
            .map(|s| s.style.fg == Some(theme::WARN))
            .collect();
        assert_eq!(
            marked,
            [false, true, false, false, false, false, false, false]
        );
    }

    #[test]
    fn a_frame_stops_at_a_breakpoint_and_resumes_past_it() {
        let mut gba = gba();
        let breakpoints = Breakpoints::from([0x0800_0004]);
        let ran = run_frame(&mut gba, &breakpoints, false);
        assert_eq!(
            ran,
            Ran {
                frame: false,
                breakpoint: true
            }
        );
        assert_eq!(gba.cpu.next_pc(), 0x0800_0004);
        assert_eq!(gba.cpu.regs.get(0), 1, "the instruction there has not run");
        // Standing on it, a frame that is not resuming stops at once.
        assert!(run_frame(&mut gba, &breakpoints, false).breakpoint);
        assert_eq!(gba.cpu.regs.get(0), 1);
        // Resuming runs it, and the loop comes back round to it.
        let ran = run_frame(&mut gba, &breakpoints, true);
        assert!(ran.breakpoint);
        assert_eq!(
            gba.cpu.regs.get(0),
            2,
            "the add ran, and the branch came back to it"
        );
        assert_eq!(gba.cpu.next_pc(), 0x0800_0004);
    }

    #[test]
    fn steps_stop_at_breakpoints_but_always_move() {
        let mut gba = gba();
        let breakpoints = Breakpoints::from([0x0800_0000, 0x0800_0004]);
        let ran = run(&mut gba, Step::Frame, &breakpoints);
        assert!(
            ran.breakpoint,
            "the one at the PC is passed, the next one stops it"
        );
        assert_eq!(gba.cpu.next_pc(), 0x0800_0004);
        let ran = run(&mut gba, Step::Instruction, &breakpoints);
        assert!(!ran.breakpoint, "an instruction step just steps");
    }

    #[test]
    fn b_toggles_at_the_pc_or_a_typed_address() {
        let gba = gba();
        let mut panel = DebuggerPanel::open();
        press(&mut panel, &gba, KeyCode::Char('b'));
        assert_eq!(
            press(&mut panel, &gba, KeyCode::Enter),
            Some(Action::Toggle(0x0800_0000))
        );
        press(&mut panel, &gba, KeyCode::Char('b'));
        for c in "8000006".chars() {
            press(&mut panel, &gba, KeyCode::Char(c));
        }
        assert_eq!(
            press(&mut panel, &gba, KeyCode::Enter),
            Some(Action::Toggle(0x0800_0004)),
            "aligned to an ARM instruction"
        );
        let lines = code_lines(&gba, 0x0800_0000, 2, &Breakpoints::from([0x0800_0004]));
        assert!(
            text(&lines[0]).starts_with(" ▶ 08000000"),
            "{}",
            text(&lines[0])
        );
        assert!(
            text(&lines[1]).starts_with("●  08000004"),
            "{}",
            text(&lines[1])
        );
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
