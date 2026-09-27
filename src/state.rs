/// Defines the various states the monkey companion can be in.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PetState {
    Idle,
    /// Held by the cursor (mochi stretch).
    Dragged,
    /// Walking to the cursor after inactivity, or chasing it after a fast flick.
    Hunting,
    /// Head being stroked by the cursor: purring.
    Petted,
    /// Kneading a tiny keyboard in time with real keystrokes.
    Typing,
    /// Peeling (and then eating) a banana while the user scrolls.
    Snacking,
    /// Short reaction to a click, or to catching the cursor.
    Booped,
    /// Thrown by a fast drag release; falls to the floor.
    Dramatic,
    MicroWiggle,
}
