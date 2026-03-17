//! Agent session registry
//!
//! Manages global agent sessions and cancellation tokens.

use oxide_agent_core::agent::{
    AgentExecutor, RuntimeContextInbox, RuntimeContextInjection, SessionId,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Global session registry for agent executors.
pub struct SessionRegistry {
    sessions: RwLock<HashMap<SessionId, Arc<RwLock<AgentExecutor>>>>,
    cancellation_tokens: RwLock<HashMap<SessionId, Arc<CancellationToken>>>,
    runtime_context_inboxes: RwLock<HashMap<SessionId, RuntimeContextInbox>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            cancellation_tokens: RwLock::new(HashMap::new()),
            runtime_context_inboxes: RwLock::new(HashMap::new()),
        }
    }

    /// Get existing session or create new one using factory
    pub async fn get_or_create<F>(&self, id: SessionId, factory: F) -> Arc<RwLock<AgentExecutor>>
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
        let built_executor = factory();
        let inbox = built_executor.runtime_context_inbox();
        let executor = Arc::new(RwLock::new(built_executor));
        let token = Arc::new(CancellationToken::new());

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id, executor.clone());
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.insert(id, token);
        }

        {
            let mut inboxes = self.runtime_context_inboxes.write().await;
            inboxes.insert(id, inbox);
        }

        executor
    }

    /// Get session if exists
    pub async fn get(&self, id: &SessionId) -> Option<Arc<RwLock<AgentExecutor>>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Check if session exists
    pub async fn contains(&self, id: &SessionId) -> bool {
        let sessions = self.sessions.read().await;
        sessions.contains_key(id)
    }

    /// Insert a session directly
    pub async fn insert(&self, id: SessionId, executor: AgentExecutor) {
        let inbox = executor.runtime_context_inbox();
        let executor_arc = Arc::new(RwLock::new(executor));
        let token = Arc::new(CancellationToken::new());

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id, executor_arc);
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.insert(id, token);
        }

        {
            let mut inboxes = self.runtime_context_inboxes.write().await;
            inboxes.insert(id, inbox);
        }
    }

    /// Queue additional user context for the next safe iteration boundary.
    pub async fn enqueue_runtime_context(&self, id: &SessionId, content: String) -> bool {
        let inboxes = self.runtime_context_inboxes.read().await;
        if let Some(inbox) = inboxes.get(id) {
            inbox.push(RuntimeContextInjection { content });
            return true;
        }

        false
    }

    /// Check if a task is currently running for this session
    pub async fn is_running(&self, id: &SessionId) -> bool {
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
    pub async fn cancel(&self, id: &SessionId) -> bool {
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
    pub async fn renew_cancellation_token(&self, id: &SessionId) {
        let mut tokens = self.cancellation_tokens.write().await;
        if let Some(id) = tokens.keys().find(|k| *k == id).cloned() {
            tokens.insert(id, Arc::new(CancellationToken::new()));
        }
    }

    /// Get the cancellation token for a session
    pub async fn get_cancellation_token(&self, id: &SessionId) -> Option<Arc<CancellationToken>> {
        let tokens = self.cancellation_tokens.read().await;
        tokens.get(id).cloned()
    }

    /// Reset a session (clear memory, todos, status)
    ///
    /// Returns `Ok(())` if reset succeeded, `Err` if session is busy
    pub async fn reset(&self, id: &SessionId) -> Result<(), &'static str> {
        self.with_executor_mut(id, |executor| {
            Box::pin(async move {
                executor.reset();
            })
        })
        .await?;
        info!(session_id = ?id, "Session reset");
        Ok(())
    }

    /// Execute a mutable action on the session executor without waiting for a running task.
    ///
    /// Returns `Err` if the session is missing or busy.
    pub async fn with_executor_mut<F, T>(
        &self,
        id: &SessionId,
        action: F,
    ) -> Result<T, &'static str>
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
    pub async fn remove(&self, id: &SessionId) {
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(id);
        }

        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(id);
        }

        {
            let mut inboxes = self.runtime_context_inboxes.write().await;
            inboxes.remove(id);
        }
    }

    /// Remove a session only if it is currently idle.
    ///
    /// Returns `true` when the session and token were removed, `false` otherwise.
    pub async fn remove_if_idle(&self, id: &SessionId) -> bool {
        let mut sessions = self.sessions.write().await;
        let mut tokens = self.cancellation_tokens.write().await;
        let mut inboxes = self.runtime_context_inboxes.write().await;

        let Some(executor_arc) = sessions.get(id).cloned() else {
            return false;
        };

        let is_running = match executor_arc.try_read() {
            Ok(executor) => executor.session().is_processing(),
            Err(_) => true,
        };

        if is_running {
            return false;
        }

        sessions.remove(id);
        tokens.remove(id);
        inboxes.remove(id);
        true
    }

    /// Clear all todos for a session
    pub async fn clear_todos(&self, id: &SessionId) -> bool {
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

#[cfg(test)]
mod tests {
    use super::SessionRegistry;
    use oxide_agent_core::agent::{AgentExecutor, AgentSession, SessionId};
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::LlmClient;
    use std::sync::Arc;

    fn build_executor(session_id: SessionId) -> AgentExecutor {
        let settings = Arc::new(AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let session = AgentSession::new(session_id);
        AgentExecutor::new(llm, session, settings)
    }

    #[tokio::test]
    async fn remove_if_idle_removes_session_and_token() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(101_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;

        let removed = registry.remove_if_idle(&session_id).await;

        assert!(removed);
        assert!(!registry.contains(&session_id).await);
        assert!(registry.get_cancellation_token(&session_id).await.is_none());
    }

    #[tokio::test]
    async fn remove_if_idle_does_not_remove_running_session() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(202_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;

        let executor_arc = registry
            .get(&session_id)
            .await
            .expect("session must exist for running-state test");

        {
            let mut executor = executor_arc.write().await;
            executor.session_mut().start_task();
        }

        let removed = registry.remove_if_idle(&session_id).await;

        assert!(!removed);
        assert!(registry.contains(&session_id).await);
        assert!(registry.get_cancellation_token(&session_id).await.is_some());
    }

    #[tokio::test]
    async fn enqueue_runtime_context_updates_session_inbox() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(303_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;

        assert!(
            registry
                .enqueue_runtime_context(&session_id, "extra context".to_string())
                .await
        );

        let executor_arc = registry
            .get(&session_id)
            .await
            .expect("session must exist for runtime context test");
        let mut executor = executor_arc.write().await;
        let pending = executor.session_mut().drain_runtime_context();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "extra context");
    }
}
