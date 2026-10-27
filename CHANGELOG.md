# Changelog

All notable changes to tuiba. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `tuiba --version` (or `-V`) prints the version.
- Colour themes: a `theme` file in the config directory picks a preset
  (`dark`, `light`, or `terminal` for the terminal's own colours) and
  overrides single colours as `#rrggbb`. (#84, #99) `t` in the library
  switches between the presets and remembers the choice.
- More ways to install: a Homebrew tap, a Scoop bucket and a Nix flake
  (`nix run github:Huseynteymurzade28/tuiba`), listed in the README. (#85)

### Changed
- The default keys for the buttons follow mGBA and most emulators,
  all within reach of one hand: `X` = A, `Z` = B, `A` = L, `S` = R.
  `B`, `L` and `R` still work too; a `keys` file brings back the old
  layout (`a = a z`). (#93)

### Fixed
- On terminals that cannot report key releases (no Kitty keyboard
  protocol), a tapped key no longer stays held for half a second: it
  now counts as held for 150 ms after its last event, so taps and
  letting go of a held key no longer overshoot. Holding a key reads as
  a short press, a pause until the terminal's auto-repeat starts, then
  a steady hold. (#92)

## [0.16.0] – 2026-10-20

A debugger in the terminal.

### Added
- A debugger panel on `F7`: registers, flags, the PPU's position and
  the code around the PC of the paused game, beside the game screen,
  with stepping by instruction, scanline or frame. (#82)
- The debugger's memory view (`Tab`): eight bytes a row with their
  text, the bytes the last step changed marked; `g` jumps the code or
  memory view to an address or a register's value. (#82)
- Breakpoints in the debugger (`b`): a game that reaches one stops
  before the instruction there and opens the debugger; scanline and
  frame steps stop at them too. (#82)

## [0.15.2] – 2026-10-20

### Fixed
- On Windows a held key stays held until it is let go. Windows consoles
  report every key release, but tuiba treated them like terminals that
  report none and let a held key drop after half a second, before the
  first auto-repeat — a brief stutter when holding a direction. (#24)

### Changed
- CI now runs the real terminal interface on Linux, macOS and Windows
  (ConPTY), driving the library, a game, the keypad and the way out. (#24)

## [0.15.1] – 2026-10-19

### Fixed
- A headless run whose output is cut short (`| head`) now stops quietly
  instead of reporting a broken pipe as an error and writing it to the
  crash log.

## [0.15.0] – 2026-10-19

Tools for looking inside a running game.

### Added
- `--disasm [arm:|thumb:]START..END` (or `START+LENGTH`) disassembles
  memory after a headless run. `tuiba-core` has the disassembler as
  `cpu::disasm::{arm, thumb}`; it decodes through the CPU's own decoder,
  so it shows what the emulator runs. (#22)
- `--profile N` counts where the CPU spends its cycles over the last N
  frames of a headless run: how much of the time it ran at all, and the
  busiest addresses with their instruction and most common return
  address. (#22)
- `--trace-io LIST` logs every write to the chosen I/O registers during
  a headless run, with frame, scanline, cycle and the instruction or DMA
  channel that made it. LIST takes register names, groups (`lcd`,
  `sound`, `dma`, `timers`, `serial`, `irq`, `all`) and hex offsets.
  `tuiba-core`: `Gba::trace_io` and `Gba::take_io_writes`. (#22)

## [0.14.0] – 2026-10-13

A faster picture, and an empty link port.

### Added
- The link port answers as an empty one instead of plain memory: an
  internally clocked transfer finishes with all ones and the serial
  interrupt, multiplayer mode reports a lone child, and `RCNT` powers
  up in general-purpose mode. Games that probe for a link cable on
  boot see none. (#25)

### Changed
- Faster scanline rendering: text backgrounds are drawn a tile row at a
  time and windows are resolved once per line. Headless speed on the
  homebrew test set is up 12–28 %, with identical frames. (#7)

## [0.13.0] – 2026-10-12

Your own BIOS, if you have one.

### Added
- Real BIOS images: `--bios FILE`, or `gba_bios.bin` in the config
  directory for every game. Games boot through the logo and chime and
  every system call runs on the real thing; `--skip-intro` goes straight
  to the game and `--no-bios` back to the built-in replacement. The
  console's and the Nintendo DS's dumps are recognised; other images
  run with a note in the status bar. (#14)
- CI runs the test ROMs on Cult-of-GBA's replacement BIOS too, with and
  without its intro, and expects the same frames as without a BIOS.

### Changed
- Save states never contain the BIOS image. Existing states still load.
  A state taken while a real BIOS was running needs one to be loaded
  back, and says so instead of crashing.
- `tuiba-core`: `Gba::with_bios(cartridge, image, Boot)` replaces
  `Gba::load_bios`, and `Gba::restore` returns a `Result`
  (`GbaError::StateNeedsBios`).

## [0.12.0] – 2026-10-08

Cheats, and a pad button for rewind.

### Added
- Cheats: `<rom>.cht` next to the ROM, in libretro's format (so
  RetroArch and libretro-database cheat files work after a rename), and
  `F6` to turn them on and off in a game. Raw, GameShark v1/v2, Action
  Replay v3 and CodeBreaker codes, encrypted or not, are told apart by
  their shape. ROM-patch and device-button codes are skipped with a
  note; re-keyed (`DEADFACE`) and encrypted CodeBreaker codes are not
  supported yet. (#43)

### Changed
- Pad defaults: `LT`/L2 held rewinds, mirroring `RT`/R2 fast-forward,
  and the save-state panel moved from L2 to the west face button (`X`
  on Xbox), which then saves inside the panel. A second press of it
  within 300 ms of opening is ignored, so a double tap cannot
  overwrite a slot. (#45)

## [0.11.0] – 2026-10-07

Two quality-of-life additions: play zipped ROMs as they are, and see
games in the colours their artists saw.

### Added
- ROMs inside `.zip` archives: the library lists them and plays them
  without unpacking, and `tuiba game.zip` works like `tuiba game.gba`.
  A lone ROM in an archive is named after the archive, so its save,
  patch and save states sit next to it as `game.sav`, `game.bps`, …
  (#41)
- LCD colour correction: `C` in a game, or `--color-correction`, shows
  colours as the GBA's screen did — darker and less saturated — in
  every renderer. Screenshots keep the raw colours. (#42)

## [0.10.0] – 2026-10-01

Accuracy work: nothing new to press, but games that do raster tricks
look more like they do on hardware.

### Added
- Open bus and BIOS read protection. Reads from unmapped addresses
  return what the bus last carried, and the BIOS reads back as the
  hardware leaves it once a game is running; jsmolka's `bios.gba` now
  passes in CI. (#13, #48)
- Affine backgrounds keep their own reference points, reloaded at
  VBlank and when a game writes them, and stepped line by line, so
  perspective effects that stream new values per line or bend the
  walk mid-frame draw correctly. (#12, #49)
- DMA runs alongside the rest of the machine instead of all at once:
  an HBlank or sound DMA during a long copy now happens on every line,
  at its own moment. (#11, #50)
- Writes to the screen's registers, palette, VRAM or OAM while a line
  is being drawn change only the pixels after them. (#11, #51)
- The per-line limit on sprite drawing: sprites past the hardware's
  cycle budget are cut off, as on the console. (#11, #52)

### Changed
- Save states carry the new state in trailing sections; files from
  earlier versions still load.
- Headless runs are a few percent faster.

## [0.9.0] – 2026-09-25

### Added
- Windows builds. Each release now carries a zip with `tuiba.exe` for
  64-bit Windows, with the C runtime built in, so nothing else needs
  installing. Run it in Windows Terminal. (#47)
- Performance figures. `--stats` or `F3` shows in the status bar the
  emulated and the shown frame rate, how long drawing a frame takes
  (average and worst over the last second), how much sound is queued
  and how often it ran dry: what to include in a report when a game
  stutters or crackles. (#47)
- A speed limit for fast-forward. `--fast-speed 2` to `16`, or `max`
  (the default), and `F4` in a game cycles ×2, ×4 and uncapped. (#44)

### Changed
- The README says what building on Windows really needs (the MSVC
  toolchain and the Visual Studio Build Tools), and points to the
  prebuilt binaries.

## [0.8.0] – 2026-09-25

### Added
- Cartridge real-time clock. The S-3511 clock that some games read over
  the cartridge's GPIO port is emulated and shows your local time, so
  day and night follow the real clock. Save states keep older files
  loadable. Headless runs start the clock at 2000-01-01 00:00:00 and
  advance it per frame; `--clock YYYY-MM-DDTHH:MM:SS` sets it. (#40)
- Volume steps. `-` and `=` / `+` change the volume in 10 % steps
  (keys-file actions `quieter` and `louder`), `--volume PERCENT` sets
  where it starts, and it carries over from game to game. (#46)
- CI runs jsmolka's gba-tests headlessly on every change and compares
  the frames they end on. (#39)

### Fixed
- Stutter and loud crackling sound on slow terminals (reported on
  Windows Terminal). A slow draw used to slow the whole game down, and
  the sound, arriving slower than it was played, broke up into a buzz.
  Frames a slow draw misses are now emulated without being drawn, so the
  game and its sound keep real time, the picture skips frames instead,
  and the sound queue adapts to small clock differences and turns a
  real stall into a short gap. (#46)
- `--mute` still opens the sound device, so `M` can turn sound on. (#46)
- CPU and bus fixes found by the test ROMs: a test instruction with
  `Rd = r15` restores CPSR from SPSR, LDM/STM keep the low bits of an
  unaligned base on writeback, and SRAM sees the exact address of
  halfword and word accesses. (#39)

## [0.7.0] – 2026-09-25

### Added
- Rewind. Hold `W` to play the last minute or so backwards, at twice the
  speed it was played, and let go to carry on from there. It works while
  paused too. States are kept as small deltas against the newest one, in
  64 MiB at most, and are dropped when you leave the game. (#38)
- ROM patches. A `.bps`, `.ups` or `.ips` file next to the ROM with the
  same name is applied in memory when the game starts; the ROM file is
  never changed. BPS and UPS checksums are verified, so a patch for a
  different revision of the game is refused with a message. Save states
  follow the patched ROM and never mix with the original's. (#38)
- Screenshots. `F12` saves the frame on screen as a 240×160 PNG in
  `tuiba` under your pictures folder, numbered after the ROM. (#38)

## [0.6.0] – 2026-09-25

### Added
- Pixels beyond Kitty. The screen can now go out as Sixel (foot, Windows
  Terminal, mintty, mlterm, Konsole, xterm as a VT340) or as iTerm2 inline
  images, so more terminals get real pixels instead of half-blocks. The
  protocol is picked from the environment; `--renderer` names one
  yourself (`auto`, `kitty`, `sixel`, `iterm2` or `blocks`), and
  `--no-graphics` stays as a shorthand for `blocks`. The status bar shows
  which one is in use. (#21)

### Changed
- A frame identical to the one on screen is not sent again, so a paused
  or still game costs the terminal nothing.
- Headless `--screenshot` PNGs are compressed now: a few dozen KiB
  instead of 115 KiB.

### Fixed
- The game screen's title falls back to the file name for placeholder
  header titles (`ROM TITLE` and friends), as the library already did.

## [0.5.0] – 2026-09-23

### Added
- Gamepads. Any pad the OS knows plays alongside the keyboard, and one
  plugged in mid-game just starts working (🎮 in the status bar). In a
  game the layout goes by position — A right, B bottom, L/R on the
  shoulders, D-pad or left stick to steer — with R2 fast-forward, L2 the
  save-state panel, north pause, the guide button twice back to the
  library and the right stick click for the bindings. The library and
  the save-state panel work from the pad too, confirming and backing out
  the way the pad's brand does. While a pad is connected every hint names
  its buttons as printed on it (Xbox, PlayStation, Nintendo). The `keys`
  file binds pad buttons (`a = z pad:east`) and two new actions, `leave`
  and `help`; a line only replaces the kinds of input it names, so
  existing keyboard layouts keep the pad defaults. Behind the default
  `gamepad` feature; on Linux it needs udev.
  `cargo run -p tuiba --example padlog` shows what a pad sends. (#37)

## [0.4.0] – 2026-09-22

### Added
- Save states: `F5` freezes the whole machine into the current slot and
  `F8` restores it, from anywhere in a game. `F2` opens a panel with the
  cartridge's four slots, each showing the frame it was taken on and how
  long ago — load, overwrite or delete from there. States are written to
  the state directory (`~/.local/state/tuiba/states`,
  `%LOCALAPPDATA%\tuiba\states` on Windows) under the cartridge's
  fingerprint, so they outlive the session and cannot be restored into
  the wrong game. (#15)

### Changed
- The crash log's directory now follows the platform convention on
  Windows as well (`%LOCALAPPDATA%`), the same way the config directory
  does.

### Fixed
- Leaving a game really does take two presses of `Esc`. tuiba now asks
  the terminal to send `Esc` as its own escape sequence rather than a
  bare `\x1b`, so a press can be told from a release; without that, one
  press of `Esc` reached the emulator as two indistinguishable events
  (measured at 79 ms apart in Kitty) and left the game on its own. Where
  a terminal reports no releases at all, two events have to be at least
  250 ms apart to count as two presses.

- Windows: the library, recents and key bindings are stored under
  `%APPDATA%\tuiba` instead of `%USERPROFILE%\.config\tuiba`.
  `XDG_CONFIG_HOME` still wins wherever it is set. (#36)

## [0.3.0] – 2026-09-22

### Added
- Sound: the four PSG channels (pulse with sweep and envelope, wave RAM,
  noise) and both direct-sound FIFOs fed by timers 0/1 and DMA 1/2, mixed
  at 32 768 Hz and played through the default audio device. `M` mutes,
  `--mute` starts muted, and the headless mode's `--wav FILE` captures a
  run's audio. (#9)

### Changed
- Leaving a game takes `Esc` twice (the first arms a two-second "esc again
  to leave" hint), and `Esc`/`q` in the library ask "Quit tuiba?" before
  quitting. `Ctrl+Q` still quits at once.

### Fixed
- The terminal no longer dies with SIGBUS (taking the emulator with it)
  while a game runs or the window is resized: every frame sent through
  the Kitty graphics file transport now goes to a fresh file instead of
  truncating one the terminal may still be reading.
- A held `Esc` on the way out of a game no longer quits the library too.

## [0.2.0] – 2026-09-22

### Added
- Pause (`P`), single-frame advance (`.`) and fast-forward (`Tab` / `F`
  held) with the multiplier in the status bar. (#16)
- Configurable key bindings in `~/.config/tuiba/keys`; `?` in a game shows
  the active bindings. (#17)
- Library: filter as you type (`/`), sort by title, file name, last played
  or size (`s`), subfolders searched three levels deep, and the last played
  cartridge is remembered and preselected. (#18)
- Saves are written within a second of the game changing backup memory,
  atomically, instead of only when leaving the game.
- Crashes are caught: the save is flushed, the library stays open, and the
  message with a backtrace lands in `~/.local/state/tuiba/crash.log`.
- Continuous integration (fmt, clippy, tests on Linux, macOS and Windows)
  and release binaries for Linux (x86_64, aarch64) and macOS (arm64,
  x86_64) on every tag. (#23)
- README: scope and legal section, new logo, badges, new recording. (#8)

### Changed
- Placeholder header titles (`ROM TITLE`, `GAME TITLE`, …) and the game
  code `0000` are treated as absent; such cartridges list under their file
  name. Thanks to @voidstackloop for the first outside contribution. (#20)
- The declared minimum Rust version is 1.88 (ratatui 0.30 requires it;
  1.85 never built).

### Fixed
- The library screen works on Windows (home directory lookup).

## [0.1.1] – 2026-09-21

### Changed
- Library screen redrawn: legible wordmark, rounded panes, cartridge
  label strip.

## [0.1.0] – 2026-09-21

First release: ARM7TDMI, memory map with DMA and timers, PPU modes 0–5
with sprites, windows and blending, HLE BIOS, SRAM/flash/EEPROM saves,
Kitty graphics and half-block renderers, library screen, headless mode.

[0.16.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.15.2...v0.16.0
[0.15.2]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.15.1...v0.15.2
[0.15.1]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.15.0...v0.15.1
[0.15.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Huseynteymurzade28/tuiba/releases/tag/v0.1.0
