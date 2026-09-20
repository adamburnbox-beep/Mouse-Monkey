# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-09-20

First release. The prototype was restructured into a library plus a thin
binary, the Windows driver was written from scratch, and the whole behaviour
layer was made testable.

### Added

- `--check`, `--self-test`, `--print-config`, `--config`, `--verbose`,
  `--help` and `--version` command line flags.
- A headless platform driver, so the engine runs (and is tested) on machines
  with no display.
- `[behaviour]` and `[animation]` configuration sections: every threshold from
  the spec and the mapping from behaviours to sprite-sheet cells are now
  configurable, with defaults matching the bundled sheet.
- Configuration discovery in `$XDG_CONFIG_HOME` / `%APPDATA%`, the working
  directory and the executable's directory; sprite paths resolve relative to
  the config file.
- Clean shutdown on `SIGINT` / `SIGTERM`, including joining input threads and
  removing Windows hooks.
- 73 unit and integration tests covering the state machine, monitor maths,
  configuration, asset validation, the animation table and the engine loop.
- CI building, testing, linting and format-checking on Linux and Windows.

### Changed

- The behaviour engine no longer depends on platform types or reads the wall
  clock; it is driven by events, a delta time and a snapshot of the world.
- Behaviour constants are expressed per second rather than per frame, so
  `tick_rate_hz` changes smoothness without changing behaviour.
- Drag squash/stretch follows frame-to-frame cursor movement and relaxes back
  to rest, instead of being pinned to the total distance from the grab point.
- Flick detection runs continuously during a drag over a moving-average window,
  so a mid-drag flick drops the monkey immediately.
- Rendering only clears and damages the rectangles that changed, instead of the
  full surface every frame.
- The Windows overlay is drawn with `UpdateLayeredWindow` from a DIB section,
  giving true per-pixel alpha.
- Sprite sheet loading returns an error naming the path and the mismatch rather
  than panicking.
- Configuration is validated at startup; unknown keys are reported.
- The `image` dependency is limited to the PNG decoder.
- Release profile: thin LTO, one codegen unit, symbols stripped, panics abort.

### Fixed

- The Windows driver did not compile at all: missing and misplaced imports,
  wrong handle types, a reference to a non-existent field, and trait methods
  whose bodies returned the wrong type. It now compiles and renders.
- A monkey flung with no monitor layout known fell forever; falls now end.
- Arriving at the cursor during a hunt immediately re-triggered the hunt,
  leaving the monkey twitching between walking and standing.
- Cursor velocity was never reset when the pointer stopped, so "the cursor is
  still" was never true and grooming could not start.
- The first cursor sample was turned into an enormous velocity, which
  instantly flung the monkey on the first click.
- Micro-wiggle and scratch timers used wall-clock instants and drifted.
- Input threads blocked on a read and could not be stopped at shutdown.
- Pointer coordinates ignored the output origin on multi-monitor layouts.
- Sprite sheet cells could be requested outside the texture.

### Removed

- 1.5 GB of build artifacts, stray duplicate sources, scratch binaries and log
  files that had been committed to the repository.
- Dead trait methods that no caller used and no driver implemented meaningfully.

[1.0.0]: https://github.com/adamburnbox-beep/mouse-monkey/releases/tag/v1.0.0
