/// Defines the various states the monkey companion can be in.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PetState {
    /// Resting near the cursor: subtle breathing, eye-follow, occasional wiggle.
    Idle,
    /// Smoothly walking toward the cursor with a springy walk bounce.
    Following,
    /// Being held by the mouse; squishes/stretches with vertical drag.
    Dragged,
    /// Cursor is hovering still over the monkey — gentle grooming pulse.
    Grooming,
    /// The user is typing: the monkey hops up and down, one foot at a time.
    Typing,
    /// Flung during a drag — tumbles under gravity until it hits the floor.
    Dramatic,
    /// Brief idle twitch/jiggle.
    MicroWiggle,
}
