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
| [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests) | MIT | ARM and THUMB instructions, memory mirrors and widths, save chips, three PPU demos |

`bios/bios.gba` from the same suite is left out until BIOS read
protection lands (#13): it fails its first test.

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
