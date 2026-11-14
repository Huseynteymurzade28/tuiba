# Game compatibility

This page records freely distributed homebrew games that have been tried
with tuiba. It is a practical compatibility snapshot, not a promise that
every level or feature works and not a substitute for testing on real
hardware.

**Status meanings**

- **Perfect** — no known visible or functional issues in the areas checked.
- **Playable** — the game runs and can be played, with any limitations noted.
- **Glitches** — it runs, but has known graphical, audio, timing, or gameplay issues.
- **Broken** — it does not reach playable gameplay.

A status only applies to the checks described in the row. An unrecorded
check must not be read as a pass.

## Tested games

The entries below reflect the compatibility notes currently published in
the project README. The exact tuiba build used for these earlier checks was
not recorded, so the version is deliberately listed as **Not recorded**.
Please add a version and date when repeating or extending a test.

| Game | Link | Licence information | Status | What was checked | tuiba version | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Anguna: Warriors of Virtue | [Game page](https://gauauu.itch.io/anguna) | MIT (code and assets, as credited in the project README) | Playable | Gameplay; SRAM saves | Not recorded | A boot-timing race exposed by this game is now handled like hardware. |
| Aereven Advance (jam build) | [Game page](https://dreamnoid.itch.io/aereven-advance) | Not stated on the linked game page; check the author's terms before redistribution. | Playable | Gameplay | Not recorded | The jam build is the version noted in the project README. |
| Heartwrench Advance | [Game page](https://michaelchase.itch.io/heartwrench-advance) | Not stated on the linked game page; check the author's terms before redistribution. | Playable | Gameplay; SRAM saves | Not recorded |  |
| Pliko | [Game page](https://pomettini.itch.io/pliko-gba-port) | MIT code licence is listed on the game page; verify separate asset/game distribution terms. | Playable | Gameplay | Not recorded |  |
| Butano Fighter | [Source and game](https://github.com/GValiente/butano/tree/master/games/butano-fighter) | Code: zlib; art: CC0 and CC BY-NC-SA 4.0, as credited in the project README. | Playable | Gameplay | Not recorded | Built from the Butano sources. |
| Varooom 3D | [Source and game](https://github.com/GValiente/butano/tree/master/games/varooom-3d) | Code: zlib; art: CC0 and CC BY-NC-SA 4.0, as credited in the project README. | Playable | Gameplay; software-rendered 3D racing | Not recorded |  |
| BeatBeast (jam build) | [Source and game](https://github.com/afska/beat-beast) | Code: MIT; art licences are listed in the game's `#licenses` directory. | Playable | Gameplay; 32 MiB cartridge | Not recorded |  |
| uCity Advance | [Source and game](https://github.com/AntonioND/ucity-advance) | Code: GPL-3.0; art: CC BY-NC-SA 4.0, as credited in the project README. | Playable | Gameplay; scenarios load and the city runs | Not recorded |  |

Licence notes describe the information available in the project README and
the linked project pages. They are not legal advice. A game's code, artwork,
music, and downloadable ROM can have different terms. Do not mirror or
commit a ROM unless its licence explicitly permits that use.

## Reporting compatibility

To report a game:

1. Open a [new compatibility report](https://github.com/Huseynteymurzade28/tuiba/issues/new?template=game-compatibility.yml).
2. Include the game title and a link to its official page or source.
3. State the tuiba version (release number or commit), operating system,
   terminal, and renderer.
4. Describe what you tried: boot, gameplay, saves, sound, clock/RTC, and
   any other relevant feature. Mark checks you did not perform as **not
   tested**.
5. Include steps to reproduce and a screenshot, log, or short description
   where useful. Do not attach or link commercial ROMs.

Commercial games may be discussed in reports, but their ROMs must not be
posted, committed, or added to CI. Only freely available homebrew and
test ROMs with suitable licences belong in the repository or automated
test suite.

## CI test ROMs

The automated test suite is separate from this gameplay list. It downloads
freely licensed test ROMs from pinned source revisions, verifies their
SHA-256 checksums, and compares a recorded final-frame hash. See
[`ci/test-roms/README.md`](../ci/test-roms/README.md) and the
[`ci/test-roms/manifest`](../ci/test-roms/manifest).

A game should only be added to CI when its licence permits the required
download/use, its source can be pinned to an immutable revision, and the
expected frame has been checked. A manually tested game is not automatically
a suitable CI test ROM.
