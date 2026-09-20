# Mouse Monkey

A small on-screen monkey that lives on top of your desktop. It watches your
cursor, scratches its head while you type, can be picked up and dragged around,
squashes and stretches while you hold it, tumbles to the floor if you fling it,
and comes looking for the cursor if you leave it alone for long enough.

It is a single native binary with no runtime, no game engine and no GPU
requirement: one 60 Hz loop, one sprite, one transparent click-through overlay.

| | |
|---|---|
| **Platforms** | Linux (Wayland + `wlr-layer-shell`), Windows 10/11 |
| **Binary size** | ~1.4 MB release build |
| **Peak memory** | ~11 MB headless, ~20 MB with a 1920x1080 overlay surface |
| **Runtime deps** | none on Windows; `libwayland-client` and `libxkbcommon` on Linux |

---

## Install

### From source

```sh
git clone https://github.com/adamburnbox-beep/mouse-monkey
cd mouse-monkey
cargo build --release
```

The binary lands in `target/release/monkey_companion`. It needs
`monkey_companion.toml` and the `assets/` directory beside it, or in one of the
config locations below.

Linux build requirements:

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev

# Fedora
sudo dnf install gcc pkgconf-pkg-config wayland-devel libxkbcommon-devel
```

Windows needs only a stable Rust toolchain with the MSVC target.

### Run

```sh
monkey_companion                    # run it
monkey_companion --check            # validate config and assets, then exit
monkey_companion --self-test        # simulate 10s of the engine, no display needed
monkey_companion --print-config     # dump the effective configuration
monkey_companion --config ./my.toml # use a specific config file
monkey_companion --help
```

`Ctrl-C` (or `SIGTERM`) shuts it down cleanly: input threads stop, hooks are
removed, the overlay goes away.

---

## Permissions

The companion needs to see input that is *not* aimed at its own window —
otherwise it cannot follow the cursor across the desktop or notice you typing.

**Linux.** Global input comes from `/dev/input/event*`, which is normally
restricted to the `input` group:

```sh
sudo usermod -aG input "$USER"   # then log out and back in
```

Without it the companion still runs. It prints a warning to stderr and falls
back to *window-local* tracking: it only reacts to the pointer when it is near
the sprite, and keyboard scratching is unavailable. This fallback is required
behaviour, not a bug.

**Windows.** Global input uses `WH_MOUSE_LL` / `WH_KEYBOARD_LL` hooks, which
need no special privileges. Some anti-cheat and endpoint-security products
block low-level hooks; if that happens the companion warns on stderr and keeps
running with whatever input it can get.

Key codes are used only to notice that *a* key moved. Nothing is recorded,
buffered or written anywhere.

---

## Configuration

`monkey_companion.toml` is searched for in this order:

1. the path given to `--config`
2. `$XDG_CONFIG_HOME/monkey-companion/` (Linux) or `%APPDATA%\monkey-companion\`
   (Windows)
3. the working directory
4. the directory containing the executable

Every key is optional and every key is validated — a typo is reported at
startup rather than silently ignored. See the comments in the shipped
[`monkey_companion.toml`](monkey_companion.toml) for the full list; the
`[behaviour]` section tunes every threshold in the spec, and `[animation]` maps
behaviours onto cells of the sprite sheet, so a different sheet can be dropped
in without touching the code.

### Using your own sprite sheet

Point `sprite.sheet_path` at a PNG whose width and height are whole multiples
of `frame_width` and `frame_height`; anything else is rejected at startup with
an explicit message. Then set the `[animation]` rows and columns to match your
layout. Clips that fall outside the sheet are clamped rather than crashing, and
the mismatch is logged.

---

## How it works

```
main.rs          argument parsing, logging, driver selection
  └── app.rs     the 60 Hz loop: poll input -> advance state -> draw one sprite
        ├── monkey.rs      the behaviour state machine (no OS types, no clock reads)
        ├── animation.rs   behaviour + direction -> a cell of the sheet
        ├── geometry.rs    vectors, monitor rectangles, clamping rules
        ├── sprite.rs      sprite sheet loading and validation
        ├── config.rs      config discovery, defaults, validation
        └── platform/      one driver per OS, chosen at compile time
              ├── wayland.rs   layer-shell overlay + evdev global input
              ├── win32.rs     layered window + low-level hooks
              └── headless.rs  no windowing system; used by tests and --self-test
```

The split matters: `monkey.rs` is driven entirely by an event slice, a delta
time and a snapshot of the world (cursor position + monitor rectangles). It
never reads the wall clock and never calls the OS, so every behaviour is
reproducible in a unit test, and `--self-test` runs the real engine on a
machine with no display at all.

**Rendering.** Both drivers blit the sprite into a 32-bit premultiplied-alpha
buffer and only clear and damage the rectangles that actually changed, rather
than the whole screen every frame. On Wayland the buffer is reused between
frames while the compositor is finished with it; on Windows it is a DIB section
handed to `UpdateLayeredWindow`, which gives true per-pixel alpha.

**Click-through.** On Wayland the input region is restricted to the sprite, so
clicks elsewhere reach the window underneath; on Windows `WS_EX_TRANSPARENT`
does the same. Clicks *on* the monkey still work because the global hooks see
input before it is routed to a window.

---

## Behaviour

| Ref | Behaviour | Where |
|---|---|---|
| FR-01 | Sprite sheet validated at startup; no runtime texture swaps | `sprite.rs` |
| FR-02 | Gaze follows the cursor across eight direction sectors | `monkey.rs`, `geometry.rs` |
| FR-03 | Drag with squash and stretch (squish is half the stretch) | `monkey.rs` |
| FR-04 | Walks to a cursor that has not moved for five minutes | `monkey.rs` |
| FR-05 | Grooms, with a breathing pulse, under a resting cursor | `monkey.rs` |
| FR-06 | Scratches on any key press, decaying after 750 ms | `monkey.rs` |
| FR-07 | Occasional 400 ms, +/-2 px idle twitch | `monkey.rs` |
| FR-08 | A flick drops the cursor lock and the monkey tumbles under gravity | `monkey.rs` |
| FR-09 | Clamped to the monitor layout, re-snapped on topology changes | `geometry.rs` |

Where the original spec fixed a value per frame at 60 Hz, the implementation
stores it per second instead, so changing `tick_rate_hz` changes the smoothness
and not the behaviour.

---

## Testing it by hand

Work outwards from the checks that need nothing, to the ones that need a real
desktop session.

### 1. Without a display

```sh
cargo test          # the state machine, monitor maths, config, assets, the loop
cargo run -- --check       # config and sprite sheet load
cargo run -- --self-test   # the real engine loop, headless
```

`--self-test` runs the same `App::run` path the overlay uses, against the
headless driver. If it prints a frame count and a state, the engine works; only
the windowing and input code is left to verify.

### 2. Make the slow behaviours fast

Several behaviours are deliberately rare — the hunt waits five minutes, the
idle twitch is a one-in-five roll every eight seconds. Testing those at their
shipped values is tedious, so override them. Save this as `test.toml` **in the
repository root** (relative sprite paths resolve against the config file):

```toml
[behaviour]
hunt_after_idle_secs = 5.0     # walk to the cursor after 5s, not 5 minutes
groom_hold_secs = 0.5          # groom almost immediately
wiggle_interval_secs = 2.0     # twitch every 2s...
wiggle_chance = 1.0            # ...every time
```

```sh
cargo run --release -- --config test.toml --verbose
```

`--verbose` logs every state transition, so you can see what the monkey thinks
it is doing even when the animation is ambiguous.

### 3. What to try, and what should happen

| Do this | Expect |
|---|---|
| Move the cursor in a circle around the monkey | its pose changes as you cross each of the eight directions (`gaze` in the logs) |
| Type anywhere, in any window | it scratches its head, and stops ~750 ms after you do |
| Click and drag it | it follows the pointer, stretching vertically as you pull down and squashing as you push up |
| Drag it, then flick the mouse hard and let go | it drops, tumbles, and falls to the bottom of the monitor |
| Rest the cursor on it and stop moving | after `groom_hold_secs` it grooms itself and breathes gently |
| Leave the mouse alone for `hunt_after_idle_secs` | it walks across the screen to the cursor and stops |
| Drag it towards a screen edge and release | it stays fully on screen |
| Click *next to* the monkey, on the window below | that window gets the click; the overlay is not in the way |
| Unplug or disable a monitor while it is running | it re-appears on a remaining monitor |
| Press Ctrl-C in the terminal | it exits cleanly, with a frame count |

### 4. Read the startup log

The first few lines tell you most of what you need:

```
loaded sprite sheet '...': 1024x1024, 8x8 frames of 128x128 (4.0 MiB in memory)
output topology: 1 monitor(s) [Rect { x: 0, y: 0, width: 3440, height: 1440 }]
evdev: keyboard /dev/input/event3 (AT Translated Set 2 keyboard)
evdev: pointer /dev/input/event5 (Logitech MX Master 3)
```

A `warning: global input capture is unavailable` line means the companion is in
window-local fallback mode — see [Permissions](#permissions). It will still
run, but it will only notice the pointer near the sprite and will not react to
typing at all.

### 5. Check the footprint

```sh
/usr/bin/time -v ./target/release/monkey_companion --self-test 2>&1 | grep Maximum
ps -o rss= -p "$(pgrep monkey_companion)"   # while it is actually running
```

Expect roughly 11 MB headless and around 20 MB with a 1920x1080 overlay
surface, against the 45 MB budget in the spec.

### Troubleshooting

| Symptom | Cause |
|---|---|
| `this compositor does not support wlr-layer-shell` | you are on GNOME/Mutter; try Sway, KDE, Hyprland or COSMIC |
| `cannot connect to a Wayland compositor` | running under X11 or over SSH without `WAYLAND_DISPLAY` |
| It ignores typing, and only notices the mouse nearby | not in the `input` group — see [Permissions](#permissions) |
| It never grooms | the cursor is not quite still; raise `groom_velocity_px_per_sec` |
| It gets flung when you meant to drag | lower `fling_acceleration_px_per_sec2` |
| Nothing visible at all | check `--check` passes, then run with `--verbose` and look for a configure line |

---

## Development

```sh
cargo test                                  # unit + integration tests
cargo clippy --all-targets                  # lints (CI treats warnings as errors)
cargo fmt --check
cargo check --target x86_64-pc-windows-msvc # type-check the Windows driver from Linux
```

73 tests cover the state machine, monitor maths, config parsing, asset
validation, the animation table and the engine loop. CI builds and tests on
Linux and Windows on every push.

## Known limitations

- **Wayland needs `wlr-layer-shell`.** Sway, Hyprland, KDE Plasma, COSMIC and
  wlroots compositors have it; GNOME's Mutter does not, and the companion exits
  at startup with a message saying so.
- **Multi-monitor on Wayland** relies on evdev for a true desktop-wide cursor
  position. Without it, positions come from pointer events on the overlay,
  which the compositor places on a single output.
- **X11 and macOS** have no driver. `--self-test` still exercises the engine
  there; adding a driver means implementing one trait in `platform/`.

## License

MIT — see [LICENSE](LICENSE).
