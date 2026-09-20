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
monkey_companion --self-test        # run the engine headlessly for 10 seconds
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
