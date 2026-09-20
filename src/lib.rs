//! Mouse Monkey — a small on-screen companion that watches the cursor, reacts
//! to typing, and can be picked up and thrown around the desktop.
//!
//! The crate is split into a portable core and a thin platform layer:
//!
//! - [`monkey`] holds the behaviour state machine. It has no OS dependencies
//!   and no wall-clock reads, so every requirement in `docs/PRD.md` is covered
//!   by a unit test.
//! - [`geometry`], [`animation`], [`sprite`] and [`config`] are the portable
//!   support code: monitor maths, the sheet-to-animation mapping, asset loading
//!   and configuration.
//! - [`platform`] defines the driver trait and the drivers themselves. Exactly
//!   one real driver is compiled in per target, plus a headless one for tests.
//! - [`app`] wires them together into the fixed-rate engine loop.

pub mod animation;
pub mod app;
pub mod cli;
pub mod config;
pub mod error;
pub mod geometry;
pub mod monkey;
pub mod platform;
pub mod shutdown;
pub mod sprite;
pub mod state;

pub use app::App;
pub use config::AppConfig;
pub use error::{Error, Result};
pub use monkey::Monkey;
pub use state::PetState;

/// The version of this build, taken from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
