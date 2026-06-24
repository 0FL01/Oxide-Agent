//! Pure life-mode domain types.

pub mod context_override;
pub mod event;
pub mod friction_pattern;
pub mod generation;
pub mod identity;
pub mod ids;
pub mod input;
pub mod memory_item;
pub mod principal;
pub mod run;
pub mod scopes;
pub mod support_protocol;
pub mod task_state;
pub mod turn;

pub use context_override::*;
pub use event::*;
pub use friction_pattern::*;
pub use generation::*;
pub use identity::*;
pub use ids::*;
pub use input::*;
pub use memory_item::*;
pub use principal::*;
pub use run::*;
pub use scopes::*;
pub use support_protocol::*;
pub use task_state::*;
pub use turn::*;

/// Millisecond unix timestamp used by life-mode storage rows.
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct TimestampMillis(i64);

impl TimestampMillis {
    /// Creates a timestamp wrapper.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the raw millisecond timestamp.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}
