<p align="center">
  <img src="docs/logo.svg" alt="tuiba — Game Boy Advance in your terminal" width="366">
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

<!-- Screenshots: docs/library.png and docs/game.png (homebrew titles only). -->

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

## Building

```sh
cargo build --release
cargo test
```

Stable Rust 1.85 or newer, no system dependencies.

## Usage

```sh
tuiba                  # open the library screen
tuiba ~/roms           # add a folder to the library, then open it
tuiba path/to/rom.gba  # play a cartridge directly
tuiba --no-graphics    # force the half-block renderer
```

The library remembers its folders in `~/.config/tuiba/library` (one path
per line). Saves are written next to the ROM as `.sav`.

In the library: `↑↓` select, `⏎` play, `a` add a folder, `tab` switch to the
folder list (`x` removes one), `q` quit. In a game the GBA buttons are the
keys of the same name (`A`, `B`, `L`, `R`, arrows, `Enter` = Start,
`Space` or `Backspace` = Select); `Esc` returns to the library, `Ctrl+Q` quits.

For debugging there is a headless mode that needs no terminal:

```sh
tuiba rom.gba --frames 600 --key start@400-410 --screenshot out.png
```

It runs the given number of frames with scripted input, prints CPU state and
throughput, and can dump the final frame as a PNG.

## Layout

| Crate        | Path    | Purpose                                                            |
| ------------ | ------- | ------------------------------------------------------------------ |
| `tuiba-core` | `core/` | Frontend-agnostic emulator: CPU, bus, PPU, timers, DMA, saves      |
| `tuiba`      | `tui/`  | Terminal frontend: library screen, renderers, input, headless mode |

## License

MIT
