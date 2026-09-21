<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/logo.svg" alt="tuiba — Game Boy Advance in your terminal" width="358">
</p>

<p align="center">
  A Game Boy Advance emulator that runs in your terminal, written in Rust with
  <a href="https://ratatui.rs">Ratatui</a>.
</p>

---

In terminals that speak the Kitty graphics protocol (Kitty, Ghostty,
WezTerm, Konsole) the 240×160 framebuffer is shown as real pixels, upscaled
by the largest integer factor that fits. Everywhere else it is drawn with
Unicode half-block characters and 24-bit colour, so a 240×80-cell terminal
shows the full screen at 1:1.

<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/demo.gif" alt="Adding a folder to the library and starting a game" width="720">
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/game.png" alt="Anguna: Warriors of Virtue running in tuiba" width="720">
  <br>
  <sub>The recording above uses the half-block renderer; this is the same game as the pixel renderer shows it.<br>
  Game: <a href="https://www.tolberts.net/anguna/">Anguna: Warriors of Virtue</a> © 2008 Nathan Tolbert and Chris Hildenbrand, released under the MIT License (code and assets).</sub>
</p>

## Features

| Area     | What works                                                                                                   |
| -------- | ------------------------------------------------------------------------------------------------------------ |
| CPU      | Full ARM7TDMI: every ARM and THUMB instruction, all modes, exceptions, interrupts                             |
| Timing   | Wait states from `WAITCNT`, sequential/non-sequential accesses, cartridge prefetch, per-instruction internal cycles |
| Memory   | Complete address map with mirrors, DMA (immediate, HBlank, VBlank), four timers with cascade                  |
| Video    | Modes 0–5, text and affine backgrounds, sprites (affine, double-size), windows, alpha blending, mosaic        |
| Saves    | SRAM, 64/128 KiB flash and serial EEPROM, auto-detected from the ROM and persisted as `.sav`                  |
| BIOS     | Runs without a BIOS image: `IntrWait`, `Div`, `Sqrt`, `ArcTan2`, `CpuSet`, LZ77/RL/`BitUnPack`, affine helpers are emulated in software |
| Input    | Keyboard with exact key releases on terminals that support the Kitty keyboard protocol                       |
| Frontend | Library screen with remembered folders and cartridge details, pixel or half-block rendering, headless debug mode |

Not there yet: sound, serial link, real-time clock, cycle-exact PPU/DMA
interleaving. Accurate enough for the homebrew below; not a reference
implementation.

## Compatibility

Tested with freely distributed homebrew:

| Title                        | Notes                                                                 |
| ---------------------------- | --------------------------------------------------------------------- |
| Anguna: Warriors of Virtue   | Plays; exposed a boot-timing race that is now handled like hardware   |
| Aereven Advance (jam build)  | Plays                                                                 |
| Heartwrench Advance          | Plays, SRAM saves                                                     |
| Pliko                        | Plays                                                                 |

No ROMs are needed to build or test the project.

## Installation

| Method                        | Command                                                        |
| ----------------------------- | -------------------------------------------------------------- |
| Arch Linux (AUR)              | `yay -S tuiba`                                                 |
| Any platform with Rust        | `cargo install tuiba`                                          |
| From source                   | `git clone https://github.com/Huseynteymurzade28/tuiba && cd tuiba && cargo install --path tui` |

Stable Rust 1.88 or newer; no system dependencies. Any terminal with
24-bit colour and a font that has the block characters (`▀ ▄ █`) works.
For the pixel renderer use Kitty, Ghostty, WezTerm or Konsole.

## Getting started

1. Run `tuiba`. The library is empty the first time.
2. Press `a`, type the folder that holds your `.gba` files (for example
   `~/Games/GBA`; `~` is expanded) and press `⏎`. The folder is remembered
   in `~/.config/tuiba/library`, one path per line, so you can also edit
   that file by hand.
3. Pick a cartridge with `↑`/`↓` and press `⏎` to play. `Esc` brings you
   back to the library; `Ctrl+Q` quits from anywhere.

Shortcuts:

```sh
tuiba ~/Games/GBA      # add a folder and open the library in one go
tuiba path/to/rom.gba  # play a cartridge directly, skipping the library
tuiba --no-graphics    # force the half-block renderer
```

Saves live next to the ROM as `<name>.sav`. The file is written within a
second of the game saving and again when you leave, so a crash or a closed
terminal costs at most a moment of progress. The save type (SRAM, flash,
EEPROM) is detected from the ROM.

If tuiba ever crashes, the message and a backtrace are appended to
`~/.local/state/tuiba/crash.log` (or `$XDG_STATE_HOME/tuiba/crash.log`);
please attach that to a bug report.

### Keys

| Library                          | In a game                                    |
| -------------------------------- | -------------------------------------------- |
| `↑` `↓` / `j` `k` — select       | `A` `B` `L` `R` — the buttons of the same name |
| `⏎` — play                       | arrows — D-pad                               |
| `a` — add a folder               | `Enter` — Start                              |
| `tab` — folder list, `x` removes | `Space` or `Backspace` — Select              |
| `r` — rescan folders             | `Esc` — back to the library                  |
| `q` — quit                       | `Ctrl+Q` — quit                              |

Terminals that support the Kitty keyboard protocol report key releases,
so holding and releasing buttons works exactly. Elsewhere a key counts as
held until it stops auto-repeating; the status bar shows `keys: timeout`
in that case.

### Headless mode

For debugging (and for the screenshots in this file) there is a mode
that needs no terminal:

```sh
tuiba rom.gba --frames 600 --key start@400-410 --screenshot out.png
```

It runs the given number of frames with scripted input, prints CPU state
and throughput, and can dump the final frame as a PNG.

## Building

```sh
cargo build --release   # binary in target/release/tuiba
cargo test
```

## Layout

| Crate        | Path    | Purpose                                                            |
| ------------ | ------- | ------------------------------------------------------------------ |
| `tuiba-core` | `core/` | Frontend-agnostic emulator: CPU, bus, PPU, timers, DMA, saves      |
| `tuiba`      | `tui/`  | Terminal frontend: library screen, renderers, input, headless mode |

## License

MIT
