<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/logo.svg" alt="tuiba — Game Boy Advance in your terminal" width="408">
</p>

<p align="center">
  A Game Boy Advance emulator that runs in your terminal, written in Rust with <a href="https://ratatui.rs">Ratatui</a>.
</p>

<p align="center">
  <a href="https://github.com/Huseynteymurzade28/tuiba/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Huseynteymurzade28/tuiba/ci.yml?branch=master&label=CI&style=flat-square" alt="CI"></a>
  <a href="https://crates.io/crates/tuiba"><img src="https://img.shields.io/crates/v/tuiba?style=flat-square&color=a896ff" alt="crates.io"></a>
  <a href="https://aur.archlinux.org/packages/tuiba"><img src="https://img.shields.io/aur/version/tuiba?style=flat-square&color=a896ff" alt="AUR"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-6f6888?style=flat-square" alt="MIT license"></a>
</p>

---

In terminals that speak the Kitty graphics protocol (Kitty, Ghostty,
WezTerm, Konsole) the 240×160 framebuffer is shown as real pixels, upscaled
by the largest integer factor that fits. Everywhere else it is drawn with
Unicode half-block characters and 24-bit colour, so a 240×80-cell terminal
shows the full screen at 1:1. Smaller terminals get a downscaled picture;
the status bar shows the current scale and the size needed for 1:1. Sound
plays through the default audio device (`M` mutes it).

<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/demo.gif" alt="Adding a folder, filtering the library and starting a game" width="800">
  <br>
  <sub>Add a folder, sort, filter, play. The game part shows the pixel renderer's output, as it looks in Kitty, Ghostty or WezTerm.</sub>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/halfblock.png" alt="The half-block renderer showing a game at 1:1 in a 240-column terminal" width="800">
  <br>
  <sub>The same game in a terminal without a graphics protocol: the half-block renderer at 1:1 in 240×80 cells.<br>
  Game: <a href="https://gauauu.itch.io/anguna">Anguna: Warriors of Virtue</a> © 2008 Nathan Tolbert and Chris Hildenbrand, released under the MIT License (code and assets).</sub>
</p>

## Features

| Area     | What works                                                                                                   |
| -------- | ------------------------------------------------------------------------------------------------------------ |
| CPU      | Full ARM7TDMI: every ARM and THUMB instruction, all modes, exceptions, interrupts                             |
| Timing   | Wait states from `WAITCNT`, sequential/non-sequential accesses, cartridge prefetch, per-instruction internal cycles |
| Memory   | Complete address map with mirrors, DMA (immediate, HBlank, VBlank, sound FIFO), four timers with cascade      |
| Video    | Modes 0–5, text and affine backgrounds, sprites (affine, double-size), windows, alpha blending, mosaic        |
| Sound    | All four PSG channels and both direct-sound FIFOs, mixed at 32 kHz and played through the default audio device |
| Saves    | SRAM, 64/128 KiB flash and serial EEPROM, auto-detected from the ROM; `.sav` written as you play              |
| BIOS     | Runs without a BIOS image: `IntrWait`, `Div`, `Sqrt`, `ArcTan2`, `CpuSet`, LZ77/RL/`BitUnPack`, affine helpers are emulated in software |
| Input    | Keyboard with exact key releases on terminals that support the Kitty keyboard protocol; bindings in a config file |
| Frontend | Library with folders, filter, sort and last-played memory; pixel or half-block rendering; pause, frame step and fast-forward; headless debug mode |

Not there yet: serial link, real-time clock, cycle-exact PPU/DMA
interleaving. Accurate enough for the homebrew below; not a reference
implementation.

## Compatibility

Tested with freely distributed homebrew:

| Title                        | Notes                                                                 |
| ---------------------------- | --------------------------------------------------------------------- |
| Anguna: Warriors of Virtue   | Plays, SRAM saves; exposed a boot-timing race that is now handled like hardware |
| Aereven Advance (jam build)  | Plays                                                                 |
| Heartwrench Advance          | Plays, SRAM saves                                                     |
| Pliko                        | Plays                                                                 |

No ROMs are needed to build or test the project.

## Scope and legal

tuiba is a hobby project, written to learn how the hardware works and
for the fun of seeing it run in a terminal. It is MIT-licensed and
contains no proprietary code:

- **No BIOS.** The GBA's system ROM is not included, downloaded or
  linked. The BIOS calls that games make are emulated in software from
  public documentation.
- **No games.** No ROMs are included or linked; you bring your own
  cartridges. Testing uses freely distributed homebrew and small
  hand-assembled programs in the test suite.
- **Homebrew only in this repository.** Screenshots and recordings show
  homebrew whose licence allows it, credited where they appear. Please
  keep it that way in contributions.

Game Boy Advance is a trademark of Nintendo. tuiba is not affiliated with
or endorsed by Nintendo.

## Installation

| Method                        | Command                                                        |
| ----------------------------- | -------------------------------------------------------------- |
| Arch Linux (AUR)              | `yay -S tuiba`                                                 |
| Any platform with Rust        | `cargo install tuiba`                                          |
| From source                   | `git clone https://github.com/Huseynteymurzade28/tuiba && cd tuiba && cargo install --path tui` |

Stable Rust 1.88 or newer. On Linux, sound goes through ALSA, so building
needs its headers (`alsa-lib` on Arch, `libasound2-dev` on Debian/Ubuntu,
`alsa-lib-devel` on Fedora); macOS and Windows need nothing extra. Any
terminal with 24-bit colour and a font that has the block characters
(`▀ ▄ █`) works. For the pixel renderer use Kitty, Ghostty, WezTerm or
Konsole.

## Getting started

1. Run `tuiba`. The library is empty the first time.
2. Press `a`, type the folder that holds your `.gba` files (for example
   `~/Games/GBA`; `~` is expanded) and press `⏎`. Subfolders up to three
   levels deep are searched too. The folder is remembered
   in `~/.config/tuiba/library`, one path per line, so you can also edit
   that file by hand.
3. Pick a cartridge with `↑`/`↓` and press `⏎` to play. `Esc` twice
   brings you back to the library; `Ctrl+Q` quits from anywhere. The
   library opens on the cartridge you played last
   (`~/.config/tuiba/recent`).

Both files live in tuiba's configuration directory: `~/.config/tuiba`,
or `$XDG_CONFIG_HOME/tuiba` where that is set, or `%APPDATA%\tuiba` on
Windows.

Shortcuts:

```sh
tuiba ~/Games/GBA      # add a folder and open the library in one go
tuiba path/to/rom.gba  # play a cartridge directly, skipping the library
tuiba --no-graphics    # force the half-block renderer
tuiba --mute           # start silent; M toggles sound in a game
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
| `/` — filter by title, file or code | `Enter` — Start                           |
| `s` — sort: title, file, last played, size | `Space` or `Backspace` — Select    |
| `a` — add a folder               | `P` — pause, `.` — advance one frame         |
| `tab` — folder list, `x` removes | `Tab` or `F` (held) — fast-forward           |
| `r` — rescan folders             | `M` — mute                                   |
|                                  | `?` — show the active bindings               |
| `q` — quit (asks first)          | `Esc` `Esc` — back to the library            |
|                                  | `Ctrl+Q` — quit                              |

Terminals that support the Kitty keyboard protocol report key releases,
so holding and releasing buttons works exactly. Elsewhere a key counts as
held until it stops auto-repeating; the status bar shows `keys: timeout`
in that case.

To change the in-game keys, create a `keys` file in the configuration
directory (`~/.config/tuiba/keys`, or `%APPDATA%\tuiba\keys` on
Windows), one action per line:

```ini
# button = key [key ...]      actions: up down left right a b l r
a      = j                    #          start select pause step fast mute
b      = k
select = space                # unlisted actions keep their defaults
fast   = f5 tab               # an empty right-hand side unbinds
```

Keys are single characters or `up down left right enter space backspace
tab insert delete home end pageup pagedown f1`–`f12 lshift rshift`.
Letters match either case. `Esc`, `Ctrl+Q` and `?` cannot be rebound;
`?` in a game lists what is active, and any problem in the file is
reported in the library footer.

### Headless mode

For debugging (and for the screenshots in this file) there is a mode
that needs no terminal:

```sh
tuiba rom.gba --frames 600 --key start@400-410 --screenshot out.png --wav out.wav
```

It runs the given number of frames with scripted input, prints CPU state
and throughput, and can dump the final frame as a PNG and the sound as a
WAV file.

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
