/// Job priorities, highest first (plan §4.6): interactive work (a slider
/// drag) always preempts background work (pre-rendering off-screen previews).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Background = 0,
    UserBatch = 1,
    Visible = 2,
    Interactive = 3,
}
