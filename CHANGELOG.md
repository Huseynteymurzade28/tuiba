# Changelog

All notable changes to tuiba. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Save states: `F5` freezes the whole machine and `F8` restores it, from
  anywhere in a game. The state is kept in memory for as long as the game
  is open. (#15)

### Fixed
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

[0.3.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Huseynteymurzade28/tuiba/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Huseynteymurzade28/tuiba/releases/tag/v0.1.0
