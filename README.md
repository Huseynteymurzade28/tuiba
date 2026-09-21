# tuiba

A Game Boy Advance emulator that runs in your terminal, written in Rust with
[Ratatui](https://ratatui.rs).

The 240×160 framebuffer is drawn with Unicode half-block characters and
24-bit colour, so a 240×80-cell terminal shows the full screen at 1:1.

## Layout

| Crate  | Path    | Purpose                                                        |
| ------ | ------- | -------------------------------------------------------------- |
| `tuiba-core` | `core/` | Frontend-agnostic emulator: ARM7TDMI, memory bus, PPU, timers |
| `tuiba`      | `tui/`  | Terminal frontend: Ratatui widget, Crossterm input             |

## Status

Early development. See the commit log for what exists so far.

## Building

```sh
cargo build --release
```

## Usage

```sh
tuiba                  # open the library screen
tuiba ~/roms           # add a folder to the library, then open it
tuiba path/to/rom.gba  # play a cartridge directly
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

## License

MIT
