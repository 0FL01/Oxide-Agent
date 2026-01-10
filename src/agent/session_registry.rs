//! Agent session registry
//!
//! Manages global agent sessions and cancellation tokens.
//! Transport-agnostic: works with any client (Telegram, Discord, Web, etc.)

use super::executor::AgentExecutor;
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Global session registry for agent executors
///
/// Generic over session ID type to support different transports:
/// - Telegram: `i64` (user_id)
/// - Web: `String` (session token)
/// - Discord: `u64` (user snowflake)
pub struct SessionRegistry<Id: Hash + Eq + Clone + Send + Sync + std::fmt::Debug + 'static> {
    sessions: RwLock<HashMap<Id, Arc<RwLock<AgentExecutor>>>>,
    cancellation_tokens: RwLock<HashMap<Id, Arc<CancellationToken>>>,
}

impl<Id: Hash + Eq + Clone + Send + Sync + std::fmt::Debug + 'static> Default
    for SessionRegistry<Id>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Hash + Eq + Clone + Send + Sync + std::fmt::Debug + 'static> SessionRegistry<Id> {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            cancellation_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Get existing session or create new one using factory
    pub async fn get_or_create<F>(&self, id: Id, factory: F) -> Arc<RwLock<AgentExecutor>>
    where
        F: FnOnce() -> AgentExecutor,
    {
        // Check if session exists
        {
            let sessions = self.sessions.read().await;
            if let Some(executor) = sessions.get(&id) {
                return executor.clone();
            }
        }

        // Create new session
        let executor = Arc::new(RwLock::new(factory()));
        let token = Arc::new(CancellationToken::new());

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id.clone(), executor.clone());
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.insert(id, token);
        }

        executor
    }

    /// Get session if exists
    pub async fn get(&self, id: &Id) -> Option<Arc<RwLock<AgentExecutor>>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Check if session exists
    pub async fn contains(&self, id: &Id) -> bool {
        let sessions = self.sessions.read().await;
        sessions.contains_key(id)
    }

    /// Insert a session directly
    pub async fn insert(&self, id: Id, executor: AgentExecutor) {
        let executor_arc = Arc::new(RwLock::new(executor));
        let token = Arc::new(CancellationToken::new());

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id.clone(), executor_arc);
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.insert(id, token);
        }
    }

    /// Check if a task is currently running for this session
    pub async fn is_running(&self, id: &Id) -> bool {
        let executor_arc = {
            let sessions = self.sessions.read().await;
            sessions.get(id).cloned()
        };

        let Some(executor_arc) = executor_arc else {
            return false;
        };

        let result = match executor_arc.try_read() {
            Ok(executor) => executor.session().is_processing(),
            Err(_) => true, // Lock held = task running
        };
        result
    }

    /// Cancel the current task for a session (lock-free)
    ///
    /// Returns `true` if cancellation was requested, `false` if no token found
    pub async fn cancel(&self, id: &Id) -> bool {
        let tokens = self.cancellation_tokens.read().await;
        if let Some(token) = tokens.get(id) {
            token.cancel();
            info!("Cancellation requested for session");
            true
        } else {
            warn!("No cancellation token found for session");
            false
        }
    }

    /// Renew the cancellation token for a session
    pub async fn renew_cancellation_token(&self, id: &Id) {
        let mut tokens = self.cancellation_tokens.write().await;
        if let Some(id) = tokens.keys().find(|k| *k == id).cloned() {
            tokens.insert(id, Arc::new(CancellationToken::new()));
        }
    }

    /// Get the cancellation token for a session
    pub async fn get_cancellation_token(&self, id: &Id) -> Option<Arc<CancellationToken>> {
        let tokens = self.cancellation_tokens.read().await;
        tokens.get(id).cloned()
    }

    /// Reset a session (clear memory, todos, status)
    ///
    /// Returns `Ok(())` if reset succeeded, `Err` if session is busy
    pub async fn reset(&self, id: &Id) -> Result<(), &'static str> {
        self.with_executor_mut(id, |executor| {
            Box::pin(async move {
                executor.reset();
            })
        })
        .await?;
        info!(user_id = ?id, "Session reset");
        Ok(())
    }

    /// Execute a mutable action on the session executor without waiting for a running task.
    ///
    /// Returns `Err` if the session is missing or busy.
    pub async fn with_executor_mut<F, T>(&self, id: &Id, action: F) -> Result<T, &'static str>
    where
        F: for<'a> FnOnce(&'a mut AgentExecutor) -> Pin<Box<dyn Future<Output = T> + Send + 'a>>,
    {
        let executor_arc = {
            let sessions = self.sessions.read().await;
            sessions.get(id).cloned()
        };

        let Some(executor_arc) = executor_arc else {
            return Err("Session not found");
        };

        let mut executor = executor_arc
            .try_write()
            .map_err(|_| "Cannot reset while task is running")?;
        Ok(action(&mut executor).await)
    }

    /// Remove a session from the registry
    pub async fn remove(&self, id: &Id) {
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(id);
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(id);
        }
    }

    /// Clear all todos for a session
    pub async fn clear_todos(&self, id: &Id) -> bool {
        let executor_arc = {
            let sessions = self.sessions.read().await;
            sessions.get(id).cloned()
        };

        let Some(executor_arc) = executor_arc else {
            return false;
        };

        let result = if let Ok(mut executor) = executor_arc.try_write() {
            executor.session_mut().clear_todos();
            true
        } else {
            false
        };
        result
    }

    /// Get the number of active sessions
    pub async fn len(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Check if registry is empty
    pub async fn is_empty(&self) -> bool {
        let sessions = self.sessions.read().await;
        sessions.is_empty()
    }
}

/// Type alias for Telegram-based session registry
pub type TelegramSessionRegistry = SessionRegistry<i64>;
