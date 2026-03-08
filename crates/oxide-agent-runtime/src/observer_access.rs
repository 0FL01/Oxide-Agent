//! Task-scoped observer access tokens for read-only web monitoring.

use oxide_agent_core::agent::TaskId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Opaque observer token that grants read-only access to one task stream.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct ObserverAccessToken(String);

impl ObserverAccessToken {
    /// Construct token wrapper from a raw bearer secret.
    #[must_use]
    pub fn from_secret(secret: String) -> Self {
        Self(secret)
    }

    /// Return the raw bearer secret.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ObserverAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ObserverAccessToken([REDACTED])")
    }
}

impl fmt::Display for ObserverAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Errors returned when issuing an observer access token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverAccessIssueError {
    /// Secure random source was unavailable.
    EntropyUnavailable,
}

impl fmt::Display for ObserverAccessIssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntropyUnavailable => {
                f.write_str("failed to issue observer token: secure entropy unavailable")
            }
        }
    }
}

impl std::error::Error for ObserverAccessIssueError {}

/// Successful observer access resolution result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObserverAccessGrant {
    /// Task that can be observed with this token.
    pub task_id: TaskId,
    /// Monotonic expiry deadline.
    pub expires_at: Instant,
}

/// Errors returned when resolving an observer access token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverAccessResolveError {
    /// Token does not exist.
    InvalidToken,
    /// Token existed but has expired.
    ExpiredToken,
    /// Token has been explicitly revoked.
    RevokedToken,
}

impl fmt::Display for ObserverAccessResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken => f.write_str("invalid observer token"),
            Self::ExpiredToken => f.write_str("observer token expired"),
            Self::RevokedToken => f.write_str("observer token revoked"),
        }
    }
}

impl std::error::Error for ObserverAccessResolveError {}

/// Construction options for [`ObserverAccessRegistry`].
pub struct ObserverAccessRegistryOptions {
    /// Token time-to-live.
    pub token_ttl: Duration,
}

impl Default for ObserverAccessRegistryOptions {
    fn default() -> Self {
        Self {
            token_ttl: Duration::from_secs(15 * 60),
        }
    }
}

impl ObserverAccessRegistryOptions {
    /// Build options with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// In-memory registry for issuing and resolving task-scoped observer tokens.
pub struct ObserverAccessRegistry {
    token_ttl: Duration,
    clock: Arc<dyn Clock>,
    state: RwLock<ObserverAccessState>,
}

#[derive(Default)]
struct ObserverAccessState {
    active_tokens: HashMap<ObserverAccessToken, ActiveTokenRecord>,
    task_index: HashMap<TaskId, HashSet<ObserverAccessToken>>,
    tombstones: HashMap<ObserverAccessToken, TombstoneRecord>,
}

#[derive(Clone, Copy)]
struct ActiveTokenRecord {
    task_id: TaskId,
    expires_at: Instant,
}

#[derive(Clone, Copy)]
struct TombstoneRecord {
    state: ObserverAccessResolveError,
    retain_until: Instant,
}

trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Default)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl ObserverAccessRegistry {
    /// Create a token registry with the provided options.
    #[must_use]
    pub fn new(options: ObserverAccessRegistryOptions) -> Self {
        Self::with_clock(options, Arc::new(SystemClock))
    }

    fn with_clock(options: ObserverAccessRegistryOptions, clock: Arc<dyn Clock>) -> Self {
        Self {
            token_ttl: options.token_ttl,
            clock,
            state: RwLock::new(ObserverAccessState::default()),
        }
    }

    /// Issue a new observer token scoped to the provided task.
    pub async fn issue(
        &self,
        task_id: TaskId,
    ) -> Result<(ObserverAccessToken, ObserverAccessGrant), ObserverAccessIssueError> {
        let now = self.clock.now();
        let expires_at = now + self.token_ttl;
        let token = self.generate_token()?;
        let record = ActiveTokenRecord {
            task_id,
            expires_at,
        };

        let mut state = self.state.write().await;
        state.active_tokens.insert(token.clone(), record);
        state.tombstones.remove(&token);
        state
            .task_index
            .entry(task_id)
            .or_default()
            .insert(token.clone());

        Ok((
            token,
            ObserverAccessGrant {
                task_id,
                expires_at,
            },
        ))
    }

    /// Resolve token access to a task when token is valid and active.
    pub async fn resolve(
        &self,
        token: &ObserverAccessToken,
    ) -> Result<ObserverAccessGrant, ObserverAccessResolveError> {
        let now = self.clock.now();
        let mut state = self.state.write().await;

        if let Some(record) = state.active_tokens.get(token).copied() {
            if record.expires_at <= now {
                state.remove_active_token(token, record.task_id);
                state.insert_tombstone(
                    token.clone(),
                    ObserverAccessResolveError::ExpiredToken,
                    now + self.token_ttl,
                );
                return Err(ObserverAccessResolveError::ExpiredToken);
            }

            return Ok(ObserverAccessGrant {
                task_id: record.task_id,
                expires_at: record.expires_at,
            });
        }

        if let Some(record) = state.tombstones.get(token).copied() {
            if record.retain_until <= now {
                state.tombstones.remove(token);
                return Err(ObserverAccessResolveError::InvalidToken);
            }
            return Err(record.state);
        }

        Err(ObserverAccessResolveError::InvalidToken)
    }

    /// Revoke a single observer token.
    pub async fn revoke(&self, token: &ObserverAccessToken) -> bool {
        let mut state = self.state.write().await;
        if let Some(record) = state.active_tokens.get(token).copied() {
            state.remove_active_token(token, record.task_id);
            state.insert_tombstone(
                token.clone(),
                ObserverAccessResolveError::RevokedToken,
                record.expires_at + self.token_ttl,
            );
            return true;
        }
        false
    }

    /// Revoke all active observer tokens for one task.
    pub async fn revoke_for_task(&self, task_id: TaskId) -> usize {
        let mut state = self.state.write().await;
        let Some(tokens) = state.task_index.remove(&task_id) else {
            return 0;
        };

        let mut revoked = 0;
        for token in tokens {
            if let Some(record) = state.active_tokens.remove(&token) {
                state.insert_tombstone(
                    token.clone(),
                    ObserverAccessResolveError::RevokedToken,
                    record.expires_at + self.token_ttl,
                );
                revoked += 1;
                if let Some(task_tokens) = state.task_index.get_mut(&record.task_id) {
                    task_tokens.remove(&token);
                    if task_tokens.is_empty() {
                        state.task_index.remove(&record.task_id);
                    }
                }
            }
        }
        revoked
    }

    /// Move expired active tokens into retained tombstones and prune stale tombstones.
    pub async fn cleanup_expired(&self) -> usize {
        let now = self.clock.now();
        let mut state = self.state.write().await;

        let expired_active = state
            .active_tokens
            .iter()
            .filter(|(_, record)| record.expires_at <= now)
            .map(|(token, record)| (token.clone(), record.task_id))
            .collect::<Vec<_>>();

        for (token, task_id) in &expired_active {
            state.remove_active_token(token, *task_id);
            state.insert_tombstone(
                token.clone(),
                ObserverAccessResolveError::ExpiredToken,
                now + self.token_ttl,
            );
        }

        let stale_tombstones = state
            .tombstones
            .iter()
            .filter(|(_, record)| record.retain_until <= now)
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>();

        for token in &stale_tombstones {
            state.tombstones.remove(token);
        }

        expired_active.len() + stale_tombstones.len()
    }

    /// Return active token count for one task.
    pub async fn active_tokens_for_task(&self, task_id: TaskId) -> usize {
        let state = self.state.read().await;
        state.task_index.get(&task_id).map_or(0, HashSet::len)
    }

    fn generate_token(&self) -> Result<ObserverAccessToken, ObserverAccessIssueError> {
        let mut token_bytes = [0_u8; 32];
        getrandom::fill(&mut token_bytes)
            .map_err(|_| ObserverAccessIssueError::EntropyUnavailable)?;
        let encoded = encode_hex(&token_bytes);
        Ok(ObserverAccessToken(format!("oa_{encoded}")))
    }
}

impl ObserverAccessState {
    fn insert_tombstone(
        &mut self,
        token: ObserverAccessToken,
        state: ObserverAccessResolveError,
        retain_until: Instant,
    ) {
        self.tombstones.insert(
            token,
            TombstoneRecord {
                state,
                retain_until,
            },
        );
    }

    fn remove_active_token(&mut self, token: &ObserverAccessToken, task_id: TaskId) {
        self.active_tokens.remove(token);
        if let Some(tokens) = self.task_index.get_mut(&task_id) {
            tokens.remove(token);
            if tokens.is_empty() {
                self.task_index.remove(&task_id);
            }
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Clock, ObserverAccessRegistry, ObserverAccessRegistryOptions, ObserverAccessResolveError,
    };
    use oxide_agent_core::agent::TaskId;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct TestClock {
        now: Mutex<Instant>,
    }

    impl TestClock {
        fn new(initial: Instant) -> Self {
            Self {
                now: Mutex::new(initial),
            }
        }

        fn advance(&self, duration: Duration) {
            let mut guard = match self.now.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard += duration;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            let guard = match self.now.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard
        }
    }

    fn registry_with_clock(ttl: Duration, clock: Arc<TestClock>) -> ObserverAccessRegistry {
        let options = ObserverAccessRegistryOptions { token_ttl: ttl };
        ObserverAccessRegistry::with_clock(options, clock)
    }

    #[tokio::test]
    async fn issue_generates_unique_opaque_tokens() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(300), Arc::clone(&clock));
        let task_id = TaskId::new();

        let first = registry.issue(task_id).await;
        let second = registry.issue(task_id).await;
        assert!(first.is_ok());
        assert!(second.is_ok());

        let (first_token, _) = match first {
            Ok(value) => value,
            Err(error) => panic!("failed to issue first token: {error}"),
        };
        let (second_token, _) = match second {
            Ok(value) => value,
            Err(error) => panic!("failed to issue second token: {error}"),
        };

        assert_ne!(first_token, second_token);
        assert!(first_token.secret().starts_with("oa_"));
        assert!(second_token.secret().starts_with("oa_"));
        assert!(!first_token.secret().contains(&task_id.to_string()));
    }

    #[tokio::test]
    async fn resolve_returns_task_id_before_expiry() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(300), Arc::clone(&clock));
        let task_id = TaskId::new();

        let issued = registry.issue(task_id).await;
        assert!(issued.is_ok());
        let (token, grant) = match issued {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };
        let resolved = registry.resolve(&token).await;

        assert!(
            matches!(resolved, Ok(entry) if entry.task_id == task_id && entry.expires_at == grant.expires_at)
        );
    }

    #[tokio::test]
    async fn resolve_reports_expired_and_cleanup_removes_expired_entries() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(2), Arc::clone(&clock));
        let task_id = TaskId::new();

        let issued = registry.issue(task_id).await;
        assert!(issued.is_ok());
        let (token, _) = match issued {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };
        clock.advance(Duration::from_secs(3));

        let resolve = registry.resolve(&token).await;
        assert_eq!(resolve, Err(ObserverAccessResolveError::ExpiredToken));
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::ExpiredToken)
        );
        assert_eq!(registry.active_tokens_for_task(task_id).await, 0);
        assert_eq!(registry.cleanup_expired().await, 0);
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::ExpiredToken)
        );

        clock.advance(Duration::from_secs(3));
        assert_eq!(registry.cleanup_expired().await, 1);
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn revoke_single_token_returns_revoked_state_during_retention() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(30), Arc::clone(&clock));
        let task_id = TaskId::new();

        let issued = registry.issue(task_id).await;
        assert!(issued.is_ok());
        let (token, _) = match issued {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };
        let revoked = registry.revoke(&token).await;

        assert!(revoked);
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );

        clock.advance(Duration::from_secs(29));
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );

        clock.advance(Duration::from_secs(2));
        assert_eq!(registry.cleanup_expired().await, 0);
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );

        clock.advance(Duration::from_secs(30));
        assert_eq!(registry.cleanup_expired().await, 1);
        assert_eq!(
            registry.resolve(&token).await,
            Err(ObserverAccessResolveError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn revoke_for_task_revokes_only_matching_task_tokens() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(60), Arc::clone(&clock));
        let first_task = TaskId::new();
        let second_task = TaskId::new();

        let first_a = registry.issue(first_task).await;
        let first_b = registry.issue(first_task).await;
        let second = registry.issue(second_task).await;

        assert!(first_a.is_ok());
        assert!(first_b.is_ok());
        assert!(second.is_ok());

        let (first_a, _) = match first_a {
            Ok(value) => value,
            Err(error) => panic!("failed to issue first token: {error}"),
        };
        let (first_b, _) = match first_b {
            Ok(value) => value,
            Err(error) => panic!("failed to issue second token: {error}"),
        };
        let (second, _) = match second {
            Ok(value) => value,
            Err(error) => panic!("failed to issue third token: {error}"),
        };

        let revoked = registry.revoke_for_task(first_task).await;
        assert_eq!(revoked, 2);

        assert_eq!(
            registry.resolve(&first_a).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );
        assert_eq!(
            registry.resolve(&first_b).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );
        assert!(
            matches!(registry.resolve(&second).await, Ok(entry) if entry.task_id == second_task)
        );
        assert_eq!(registry.active_tokens_for_task(first_task).await, 0);
        assert_eq!(registry.active_tokens_for_task(second_task).await, 1);
    }

    #[tokio::test]
    async fn debug_and_display_do_not_leak_secret() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(60), Arc::clone(&clock));
        let task_id = TaskId::new();

        let issued = registry.issue(task_id).await;
        assert!(issued.is_ok());
        let (token, _) = match issued {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };

        let raw = token.secret().to_string();
        let debug = format!("{token:?}");
        let display = token.to_string();

        assert!(!debug.contains(&raw));
        assert!(!display.contains(&raw));
        assert_eq!(display, "[REDACTED]");
    }

    #[tokio::test]
    async fn expired_and_revoked_states_do_not_collapse_into_invalid() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registry = registry_with_clock(Duration::from_secs(2), Arc::clone(&clock));
        let expired_task = TaskId::new();
        let revoked_task = TaskId::new();

        let expired_issue = registry.issue(expired_task).await;
        assert!(expired_issue.is_ok());
        let (expired_token, _) = match expired_issue {
            Ok(value) => value,
            Err(error) => panic!("failed to issue expired token: {error}"),
        };

        let revoked_issue = registry.issue(revoked_task).await;
        assert!(revoked_issue.is_ok());
        let (revoked_token, _) = match revoked_issue {
            Ok(value) => value,
            Err(error) => panic!("failed to issue revoked token: {error}"),
        };

        assert!(registry.revoke(&revoked_token).await);
        clock.advance(Duration::from_secs(3));

        assert_eq!(
            registry.resolve(&expired_token).await,
            Err(ObserverAccessResolveError::ExpiredToken)
        );
        assert_eq!(
            registry.resolve(&revoked_token).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );

        assert_eq!(registry.cleanup_expired().await, 0);

        assert_eq!(
            registry.resolve(&expired_token).await,
            Err(ObserverAccessResolveError::ExpiredToken)
        );
        assert_eq!(
            registry.resolve(&revoked_token).await,
            Err(ObserverAccessResolveError::RevokedToken)
        );

        clock.advance(Duration::from_secs(3));
        assert_eq!(registry.cleanup_expired().await, 2);
        assert_eq!(
            registry.resolve(&expired_token).await,
            Err(ObserverAccessResolveError::InvalidToken)
        );
        assert_eq!(
            registry.resolve(&revoked_token).await,
            Err(ObserverAccessResolveError::InvalidToken)
        );
    }
}
