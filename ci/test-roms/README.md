# Test ROMs

`run.sh` runs freely licensed test ROMs headlessly and compares a hash
of the final frame with the one recorded in `manifest`. CI runs it on
every push and pull request; locally:

```sh
cargo build --release -p tuiba
ci/test-roms/run.sh
```

The ROMs are downloaded on first use into `target/test-roms/` (or
`$TUIBA_TEST_ROMS`) from a commit pinned in the manifest, and each is
checked against its sha256 before it runs. None are committed here.

## Suites

| Source | Licence | What |
| ------ | ------- | ---- |
| [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests) | MIT | ARM and THUMB instructions, memory mirrors and widths, BIOS read protection, save chips, three PPU demos |

## BIOS

Every ROM also runs on [Cult-of-GBA's replacement BIOS](https://github.com/Cult-of-GBA/BIOS)
(MIT), once with `--skip-intro` and once booting through its intro, and
must reach the same frame as on tuiba's built-in replacement. That keeps
the real-BIOS path (`--bios`) working without Nintendo's image, which
cannot be downloaded legally. A ROM marked `hle` in the manifest is left
out of these runs — `bios.gba` checks for the exact opcodes Nintendo's
BIOS leaves on the bus.

## Adding a ROM

Only ROMs whose licence allows it — homebrew test suites, never
commercial games.

1. If the suite is new, add a `source` line: a name, the licence, and a
   URL prefix pinned to a commit (not a branch, so the ROM cannot change
   under the hash).
2. Download the ROM, take its sha256, and run it:
   `target/release/tuiba rom.gba --frames 120 --screenshot rom.png`.
3. Look at `rom.png`. Only if the frame is right — the suite says it
   passed, or a demo shows what its source says it draws — add a `rom`
   line with the `frame=` value the run printed.

If a change to the emulator alters a recorded frame on purpose (a PPU
accuracy fix, say), check the new screenshot the same way and update
the hash in the same commit.
