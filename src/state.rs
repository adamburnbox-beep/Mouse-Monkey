/// Defines the various states the monkey companion can be in.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PetState {
    Idle,
    Dragged,
    Hunting,
    Grooming,
    Scratching,
    Dramatic,
    MicroWiggle,
}