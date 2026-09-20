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
cargo run --release -- path/to/rom.gba
```

## License

MIT
