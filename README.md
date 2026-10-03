# Mouse Monkey

A tiny pixel-art monkey that lives on your desktop. He watches your cursor, types along
when you type, purrs when you pet him, and gets thrown around when you fling him.

Inspired by [Comnyang](https://comnyang.com/en), the desktop cat.

![Mouse Monkey interactions](docs/interactions.gif)

## Interactions

### Mouse

| What you do | What he does |
|---|---|
| **Move the cursor** | Follows it with his eyes, turning his head toward it in 8 directions. |
| **Click him** (quick click, don't drag) | Boop! He squeezes his eyes shut, giggles and does a little hop. |
| **Drag him** | Gets picked up by the scruff, legs dangling. He stretches like mochi as you move him, more when you move faster, and wobbles back into shape when you let go. |
| **Throw him** (let go mid-flick) | Tumbles through the air, bounces off the screen edges and lands on the bottom of the screen. |
| **Stroke his head** (move the cursor side to side over it a few times) | Closes his eyes and purrs, with little hearts floating up. Keeps purring as long as you keep petting. |
| **Whip the cursor across the screen** | Runs after it. If he catches it he celebrates, then takes a short breather before he'll chase again. |
| **Leave the mouse alone for 5 minutes** | Wanders over to wherever the cursor is resting. |
| **Scroll** | Peels a banana a little with every notch of the wheel (about 12 notches), then eats it. Stop scrolling for a few seconds and he puts it away. |

### Keyboard

| What you do | What he does |
|---|---|
| **Type anywhere** | Sits at his own tiny keyboard and presses a paw for every key, alternating left and right. The key he hits lights up. |
| **Press Space or Enter** | Thumps the keyboard with both paws. |
| **Type fast** (about 5+ keys a second) | Gets flushed and starts to sweat. |
| **Type really fast** (about 8+ keys a second) | Overheats: he turns red, squeezes his eyes shut and steam puffs from his head. He cools down again when you slow down. |
| **Stop typing** | Goes back to what he was doing after a second and a half. |

When he's idle he breathes, sways his tail and occasionally does a little wiggle.

He never gets in the way of your other windows: he only watches your input, never
takes it. Clicks and scrolls anywhere except on his body go straight to the window
underneath, even right next to him. Set `click_through = true` (see
[Configuration](#configuration)) if you'd rather he ignored clicks and scrolls on his
body too.

## Requirements

- **Linux with Wayland**, and a compositor that supports the layer-shell protocol, e.g.
  COSMIC (Pop!_OS), KDE Plasma, Sway or Hyprland. GNOME doesn't support it, so the monkey
  won't appear there.
- **Rust**: install from [rustup.rs](https://rustup.rs).
- **Build dependencies** (Debian/Ubuntu/Pop!_OS package names):
  ```bash
  sudo apt install pkg-config libxkbcommon-dev libwayland-dev
  ```
- **Access to input devices.** Wayland doesn't let apps see typing or mouse movement in
  other windows, so the monkey reads `/dev/input` directly. Add yourself to the `input`
  group, then **log out and back in** (a new terminal isn't enough):
  ```bash
  sudo usermod -aG input $USER
  ```
  To try it before logging out, start him with `sg input -c 'cargo run --release'`.
  Check it worked with `id -nG | grep input`. Without it he can still be clicked,
  dragged and petted, and scrolling on him still peels the banana, but he won't react
  to typing or to scrolling elsewhere, and he only notices the cursor when it's over
  him.

  He reads every keyboard, mouse and touchpad, including ones connected after he
  starts (e.g. Bluetooth). On a touchpad, two-finger scrolling counts as scrolling.

Windows support is not working yet.

## Running

From the project folder (the config and sprite paths are relative to it):

```bash
cargo run --release
```

He appears in the middle of the screen. Press **Ctrl+C** in the terminal to quit.

To see what he's doing and why, turn on logging:

```bash
RUST_LOG=info cargo run --release
```

At startup this lists the input devices found (`evdev: reading … as keyboard`); the
first key press prints `evdev: receiving key presses from <device>`, and state changes
print lines like `Monkey: typing along.`, `Monkey: chasing the cursor.` or
`Monkey: landed.`

## Troubleshooting

- **He doesn't react to typing.** Look at the warning printed at startup. A
  permission-denied warning says what to do: either the `input` group step above is
  missing, or you haven't logged out and back in since. If no `receiving key presses`
  line appears with `RUST_LOG=info` when you type, please open an issue with the
  `evdev:` lines from the log.
- **An app won't scroll or click where he's sitting.** Input on his body goes to him
  so you can drag him. Move him aside, or set `click_through = true`.
- **No monkey appears at all.** Your compositor probably doesn't support layer-shell
  (e.g. GNOME).

## Configuration

Settings live in `monkey_companion.toml`:

| Setting | Meaning |
|---|---|
| `tick_rate_hz` | Updates per second (default 60). |
| `click_through` | `true` makes clicks and scrolls on him pass through to the window underneath. He can't be clicked or dragged then; everything else still works through `/dev/input`. Default `false`. |
| `sprite.sheet_path` | The sprite sheet to load. |
| `sprite.frame_width`, `sprite.frame_height` | Size of one animation frame on the sheet (128 × 128). |
| `window_width`, `window_height` | Placeholder overlay size; the overlay always covers the whole screen. |

## Development

Run the tests (no display needed):

```bash
cargo test
```

The behaviour tests in `src/monkey.rs` drive the monkey with simulated mouse, keyboard
and scroll input, covering each interaction above.

### Project layout

| Path | What's there |
|---|---|
| `src/monkey.rs` | The monkey's behaviour: states, timers, physics and which animation frame to show. |
| `src/wayland.rs` | Linux driver: the transparent overlay, rendering, and reading input from `/dev/input`. |
| `src/platform.rs` | The input events and driver interface shared by all platforms. |
| `src/sprite_renderer.rs` | Loads and validates the sprite sheet. |
| `assets/sprites/` | `monkey_directional.png` (the animation sheet the app uses) and `monkey_brown.png` (a sheet of 16 emotes). |
| `tools/sprites/` | The pixel art source that generates both sprite sheets. |

### Editing the sprites

The sprites are hand-drawn pixel art written as text grids in `tools/sprites/`, where
each letter is a palette colour. After editing, rebuild the sheets (needs Python 3 and
Pillow):

```bash
sudo apt install python3-pil
python3 tools/sprites/build.py
```

The animation sheet is 8 columns × 14 rows of 128 px frames. `tools/sprites/build.py`
documents what each row is for, and the row numbers must match the `ROW_*` constants in
`src/monkey.rs`.
