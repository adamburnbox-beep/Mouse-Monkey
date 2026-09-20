# Contributing

## Getting set up

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev

cargo test
cargo run -- --self-test
```

`--self-test` runs the real engine loop against the headless driver, so it
works on a machine with no display and is the quickest way to check that a
change has not broken the loop.

## Before opening a pull request

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --target x86_64-pc-windows-msvc   # if you touched platform code
```

CI runs all of these on Linux and Windows and fails on any warning.

## Where things go

- **Behaviour changes** belong in `src/monkey.rs`. It must stay free of
  platform types and wall-clock reads: it takes events, a delta time and a
  `World`, and nothing else. That is what makes the behaviour testable.
- **New tunables** go in `src/config.rs` with a default that preserves current
  behaviour, a validation rule, and a line in `monkey_companion.toml`.
- **Sprite layout** belongs in `src/animation.rs` and the `[animation]` config
  section — never as a row or column literal in the state machine.
- **A new platform** means implementing `PlatformDriver` in a new
  `src/platform/*.rs` and adding a branch to `platform_driver()` in
  `src/main.rs`. Nothing above `platform/` should need to change.

## Testing expectations

Every behaviour change needs a test. Behaviour tests use
`Monkey::with_seed(..)` so idle randomness is deterministic, and drive the
monkey a tick at a time rather than sleeping. Tests that need a whole
application use `HeadlessDriver`.

Name tests after the behaviour they pin down, not after the function they call:
`a_still_cursor_resting_on_the_sprite_starts_grooming` says what broke when it
fails.

## Commit messages

One logical change per commit, with a subject line that says what changed and a
body that says why if it is not obvious.
