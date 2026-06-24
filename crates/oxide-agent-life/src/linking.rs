//! Explicit cross-transport identity-link token contracts.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// One-time raw link token returned to the currently authenticated user.
///
/// Only `hash_link_token(raw)` is persisted in `life_link_tokens`; transports
/// consume the raw token once to create a provider-subject identity link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLifeLinkToken(String);

impl RawLifeLinkToken {
    /// Generates a high-entropy URL/chat-safe token.
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    /// Wraps a caller-provided token for tests or Telegram consumption.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Returns the raw token string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Computes the canonical storage hash for a raw link token.
#[must_use]
pub fn hash_link_token(raw_token: &RawLifeLinkToken) -> String {
    let digest = Sha256::digest(raw_token.as_str().as_bytes());
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_token_hash_does_not_expose_raw_token() {
        let raw = RawLifeLinkToken::new("telegram-link-token");
        let hash = hash_link_token(&raw);

        assert!(hash.starts_with("sha256:"));
        assert!(!hash.contains(raw.as_str()));
        assert_eq!(hash, hash_link_token(&raw));
    }
}
