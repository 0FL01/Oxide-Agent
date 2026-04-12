//! Agent session identity types.

use std::fmt;

/// Transport-agnostic session identifier.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct SessionId(i64);

impl SessionId {
    /// Return the raw `i64` value for this session.
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for SessionId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
