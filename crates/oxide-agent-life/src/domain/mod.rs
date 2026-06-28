//! Pure life-mode domain types.

pub mod event;
pub mod identity;
pub mod ids;
pub mod input;
pub mod principal;
pub mod run;
pub mod turn;

pub use event::*;
pub use identity::*;
pub use ids::*;
pub use input::*;
pub use principal::*;
pub use run::*;
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
