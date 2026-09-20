//! The companion's behaviour states.

use std::fmt;

/// Every state the monkey can be in.
///
/// The variants map one-to-one onto the functional requirements in
/// `docs/PRD.md`, which keeps the state machine auditable against the spec.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum PetState {
    /// Resting, watching the cursor (FR-02).
    Idle,
    /// Held by the pointer, squashing and stretching (FR-03).
    Dragged,
    /// Walking towards a cursor that has not moved in a long time (FR-04).
    Hunting,
    /// Being petted by a still cursor resting on the sprite (FR-05).
    Grooming,
    /// Reacting to keyboard activity (FR-06).
    Scratching,
    /// A short idle twitch (FR-07).
    MicroWiggle,
    /// Flung by a fast drag and tumbling under gravity (FR-08).
    Dramatic,
}

impl PetState {
    /// Short lowercase name, used in logs and in the `--self-test` report.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Dragged => "dragged",
            Self::Hunting => "hunting",
            Self::Grooming => "grooming",
            Self::Scratching => "scratching",
            Self::MicroWiggle => "micro-wiggle",
            Self::Dramatic => "dramatic",
        }
    }
}

impl fmt::Display for PetState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
