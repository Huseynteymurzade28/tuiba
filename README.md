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

In terminals that can show images the 240×160 framebuffer is shown as real
pixels, upscaled by the largest integer factor that fits: through the Kitty
graphics protocol (Kitty, Ghostty, WezTerm, Konsole), Sixel (foot, Windows
Terminal, mintty, mlterm) or iTerm2's inline images. Everywhere else it is drawn with
Unicode half-block characters and 24-bit colour, so a 240×80-cell terminal
shows the full screen at 1:1. Smaller terminals get a downscaled picture;
the status bar shows the current scale and the size needed for 1:1. Sound
plays through the default audio device (`M` mutes it, `-` and `+` set the
volume).

<p align="center">
  <img src="https://raw.githubusercontent.com/Huseynteymurzade28/tuiba/master/docs/demo.gif" alt="Filtering the library, playing two homebrew games, fast-forward, save states and the key list" width="800">
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
| Clock    | The cartridge real-time clock (S-3511 over GPIO) some games use for day and night, reading your local time     |
| BIOS     | Runs without a BIOS image: `IntrWait`, `Div`, `Sqrt`, `ArcTan2`, `CpuSet`, LZ77/RL/`BitUnPack`, affine helpers are emulated in software |
| Input    | Keyboard with exact key releases on terminals that support the Kitty keyboard protocol; gamepads, hot-pluggable; bindings in a config file |
| Frontend | IPS/UPS/BPS patches applied on load; library with folders, filter, sort and last-played memory; pixels over the Kitty, Sixel or iTerm2 protocol, or half-blocks; pause, frame step, fast-forward, rewind and screenshots; headless debug mode |

Not there yet: serial link, cycle-exact PPU/DMA
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

No ROMs are needed to build or test the project. CI additionally runs
freely licensed test ROMs ([jsmolka/gba-tests](https://github.com/jsmolka/gba-tests):
ARM, THUMB, memory, save chips and PPU demos), fetched at a pinned commit;
see [`ci/test-roms`](ci/test-roms/README.md).

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
| Prebuilt (Linux, macOS, Windows) | download from [Releases](https://github.com/Huseynteymurzade28/tuiba/releases/latest) |
| Any platform with Rust        | `cargo install tuiba`                                          |
| From source                   | `git clone https://github.com/Huseynteymurzade28/tuiba && cd tuiba && cargo install --path tui` |

Stable Rust 1.88 or newer. On Linux, sound goes through ALSA and
gamepads are found through udev, so building needs their headers
(`alsa-lib` and `systemd-libs` on Arch, `libasound2-dev libudev-dev` on
Debian/Ubuntu, `alsa-lib-devel systemd-devel` on Fedora). macOS needs
nothing extra. On Windows use Rust's default MSVC toolchain, which needs
the Visual Studio Build Tools (rustup offers to install them); the
`windows-gnu` toolchain needs a full MinGW-w64 install to link. The
Windows build in Releases needs none of this: unzip it and run
`tuiba.exe` in Windows Terminal. A keyboard-only build skips udev:
`cargo install tuiba --no-default-features`. Any
terminal with 24-bit colour and a font that has the block characters
(`▀ ▄ █`) works. For the pixel renderer use Kitty, Ghostty, WezTerm,
Konsole, foot, Windows Terminal, iTerm2, mintty or mlterm. tuiba picks the
protocol from environment variables such as `TERM` and `TERM_PROGRAM`; when it
guesses wrong, name one with `--renderer` (`kitty`, `sixel`, `iterm2` or
`blocks`). Other terminals with Sixel support (xterm started with
`-ti vt340`, Contour, VS Code with images enabled) work with
`--renderer sixel`. Inside tmux or screen the escapes would need wrapping, so
tuiba falls back to half-blocks there.

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
tuiba --renderer sixel # pick the image protocol yourself
tuiba --no-graphics    # force the half-block renderer
tuiba --mute           # start silent; M toggles sound in a game
tuiba --volume 50      # start at half volume; - and + change it in a game
tuiba --stats          # frame rates, draw times and the sound queue; F3 in a game
tuiba --fast-speed 4   # fast-forward at most ×4 (default max); F4 cycles ×2, ×4, max
```

`F5` freezes the machine into the current slot and `F8` puts it back —
the whole console, not just the game's own save, so it works in
cartridges that never save and in the middle of a boss fight alike.
`F2` opens the panel: four slots, each showing the frame it was taken
on and how long ago that was, with the game paused behind it.

```
╭ SAVE STATES ──────────────────────────────────────────╮
│ ╭ 1 ─────────────────────╮   ┏▸ 2 ━━━━━━━━━━━━━━━━━━━┓ │
│ │      (the frame it     │   ┃     (…and this one)   ┃ │
│ │       was taken on)    │   ┃                       ┃ │
│ ╰ 4 minutes ago ─────────╯   ┗ just now ━━━━━━━━━━━━━┛ │
│ ╭ 3 ─────────────────────╮   ╭ 4 ─────────────────────╮ │
│ │         empty          │   │         empty          │ │
│ ╰ empty ─────────────────╯   ╰ empty ─────────────────╯ │
│           ⏎ load   s overwrite   x delete   esc close   │
╰─────────────────────────────────────────────────────────╯
```

States are written to `~/.local/state/tuiba/states/` (or
`$XDG_STATE_HOME`, or `%LOCALAPPDATA%\tuiba\states` on Windows) under
the cartridge's fingerprint, so they survive closing tuiba and can only
ever be loaded back into the game they came from.

To play a translation or a ROM hack, put its patch next to the ROM
with the same name — `game.gba` and `game.bps` — and tuiba applies it
in memory when the game starts; the ROM file itself is never changed.
BPS, UPS and IPS patches work. BPS and UPS carry checksums, so a patch
made for another revision of the game is refused with a message instead
of starting a broken game. Save states follow the patched ROM, so they
never mix with the unpatched game's.

Saves live next to the ROM as `<name>.sav`. The file is written within a
second of the game saving and again when you leave, so a crash or a closed
terminal costs at most a moment of progress. The save type (SRAM, flash,
EEPROM) is detected from the ROM.

Hold `W` to play the last minute or so backwards, at twice the speed
it was played; let go and the game carries on from there. It works while
paused too, which is the way to find the frame just before a mistake.
The rewind buffer lives in memory (64 MiB at most) and is gone when you
leave the game.

`F12` saves the frame on screen as a 240×160 PNG in `tuiba` inside your
pictures folder (`XDG_PICTURES_DIR`, else `~/Pictures`), numbered after
the ROM: `anguna-001.png`, `anguna-002.png`, …

If tuiba ever crashes, the message and a backtrace are appended to
`crash.log` in the same state directory; please attach that to a bug
report.

### Keys

| Library                          | In a game                                    |
| -------------------------------- | -------------------------------------------- |
| `↑` `↓` / `j` `k` — select       | `A` `B` `L` `R` — the buttons of the same name |
| `⏎` — play                       | arrows — D-pad                               |
| `/` — filter by title, file or code | `Enter` — Start                           |
| `s` — sort: title, file, last played, size | `Space` or `Backspace` — Select    |
| `a` — add a folder               | `P` — pause, `.` — advance one frame         |
| `tab` — folder list, `x` removes | `Tab` or `F` (held) — fast-forward, `F4` — its limit |
|                                  | `W` (held) — rewind                          |
| `r` — rescan folders             | `M` — mute, `-` / `+` — volume               |
|                                  | `F5` — save state, `F8` — load it back       |
|                                  | `F2` — the four save-state slots             |
|                                  | `F12` — screenshot                           |
|                                  | `F3` — performance figures                   |
|                                  | `?` — show the active bindings               |
| `q` — quit (asks first)          | `Esc` twice — back to the library            |
|                                  | `Ctrl+Q` — quit                              |

Terminals that support the Kitty keyboard protocol report key releases,
so holding and releasing buttons works exactly. Elsewhere a key counts as
held until it stops auto-repeating; the status bar shows `keys: timeout`
in that case.

A gamepad works as soon as it is plugged in, even mid-game, and while
one is connected the hints name its buttons as printed on it (Xbox,
PlayStation and Nintendo labels are recognised). In a game the buttons
sit where a GBA has them: A is the right face button, B the bottom one,
L and R the shoulders, and the D-pad or the left stick steer — by
position, so on an Xbox pad the GBA's A is the button labelled B. Menus
follow the pad's own habit instead: on Xbox and PlayStation pads the
bottom button confirms and the right one backs out.

| On a pad (Xbox labels)     | Does                                      |
| -------------------------- | ----------------------------------------- |
| `RT` (held)                | fast-forward                              |
| `Y`                        | pause                                     |
| `LT`                       | save-state panel: `A` load, `X` save, `Y` delete, `B` close |
| `Xbox` twice               | back to the library                       |
| `RS` (click)               | show the bindings                         |
| library: `A` / `Menu`      | play; `LB`/`RB` page through the list     |

Quick save and load stay off the pad by default, so one stray press
cannot throw progress away; bind them in the `keys` file if you want
them. Pads are read from the OS rather than the terminal, so they work in
any terminal on the same machine — not over SSH.

To change the in-game keys, create a `keys` file in the configuration
directory (`~/.config/tuiba/keys`, or `%APPDATA%\tuiba\keys` on
Windows), one action per line:

```ini
# button = key [key ...]      actions: up down left right a b l r
a      = j                    #          start select pause step fast speed rewind
b      = k pad:west           #          mute quieter louder save load states
select = space                #          screenshot stats leave help
start  = enter                # unlisted actions keep their defaults
fast   = f9 tab               # an empty right-hand side unbinds
```

Keys are single characters or `up down left right enter space backspace
tab insert delete home end pageup pagedown f1`–`f12 lshift rshift`.
Pad buttons are `pad:` and one of `south east west north l1 r1 l2 r2
select start mode lstick rstick up down left right`. A line only replaces
the kinds it names: `a = j` keeps A's pad button, `a = pad:west` keeps
its keys. Letters match either case. `Esc`, `Ctrl+Q` and `?` cannot be rebound;
`?` in a game lists what is active, and any problem in the file is
reported in the library footer.

### Headless mode

For debugging (and for the screenshots in this file) there is a mode
that needs no terminal:

```sh
tuiba rom.gba --frames 600 --key start@400-410 --screenshot out.png --wav out.wav
```

It runs the given number of frames with scripted input, prints CPU state,
throughput and a hash of the final frame, and can dump the final frame as
a PNG and the sound as a WAV file. A cartridge clock starts at
2000-01-01 00:00:00 (or `--clock 2026-09-25T21:00:00`) and advances with
the emulated frames, so runs are reproducible.

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
