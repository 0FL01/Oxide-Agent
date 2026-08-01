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
    sessions: RwLock<HashMap<SessionId, SessionEntry>>,
}

struct SessionEntry {
    executor: Arc<RwLock<AgentExecutor>>,
    cancellation_token: Arc<CancellationToken>,
    runtime_context_inbox: RuntimeContextInbox,
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
        }
    }

    /// Get session if exists
    pub async fn get(&self, id: &SessionId) -> Option<Arc<RwLock<AgentExecutor>>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).map(|entry| Arc::clone(&entry.executor))
    }

    /// Get the executor and cancellation token from one registry snapshot.
    pub async fn execution_handles(
        &self,
        id: &SessionId,
    ) -> Option<(Arc<RwLock<AgentExecutor>>, Arc<CancellationToken>)> {
        let sessions = self.sessions.read().await;
        sessions.get(id).map(|entry| {
            (
                Arc::clone(&entry.executor),
                Arc::clone(&entry.cancellation_token),
            )
        })
    }

    /// Check if session exists
    pub async fn contains(&self, id: &SessionId) -> bool {
        let sessions = self.sessions.read().await;
        sessions.contains_key(id)
    }

    /// Insert a session directly
    pub async fn insert(&self, id: SessionId, executor: AgentExecutor) {
        let runtime_context_inbox = executor.runtime_context_inbox();
        self.sessions.write().await.insert(
            id,
            SessionEntry {
                executor: Arc::new(RwLock::new(executor)),
                cancellation_token: Arc::new(CancellationToken::new()),
                runtime_context_inbox,
            },
        );
    }

    /// Queue additional user context for the next safe iteration boundary.
    pub async fn enqueue_runtime_context(&self, id: &SessionId, content: String) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(id) {
            entry
                .runtime_context_inbox
                .push(RuntimeContextInjection::text(content));
            return true;
        }

        false
    }

    /// Resume a paused session that is explicitly waiting for user input.
    ///
    /// Returns `Ok(true)` when pending input was consumed and queued,
    /// `Ok(false)` when the session exists but is not waiting for user input,
    /// and `Err` when the session is missing or currently busy.
    pub async fn resume_with_user_input(
        &self,
        id: &SessionId,
        content: String,
    ) -> Result<bool, &'static str> {
        self.with_executor_mut(id, |executor| {
            Box::pin(async move { executor.resume_with_user_input(content) })
        })
        .await
    }

    /// Check if a task is currently running for this session
    pub async fn is_running(&self, id: &SessionId) -> bool {
        let executor_arc = {
            let sessions = self.sessions.read().await;
            sessions.get(id).map(|entry| Arc::clone(&entry.executor))
        };

        let Some(executor_arc) = executor_arc else {
            return false;
        };

        match executor_arc.try_read() {
            Ok(executor) => executor.session().is_processing(),
            Err(_) => true, // Lock held = task running
        }
    }

    /// Cancel the current task for a session (lock-free)
    ///
    /// Returns `true` if cancellation was requested, `false` if the session is missing.
    pub async fn cancel(&self, id: &SessionId) -> bool {
        let token = self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| Arc::clone(&entry.cancellation_token));
        if let Some(token) = token {
            token.cancel();
            info!("Cancellation requested for session");
            true
        } else {
            warn!("No session found for cancellation");
            false
        }
    }

    /// Renew the cancellation token for a session
    pub async fn renew_cancellation_token(&self, id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(id) {
            entry.cancellation_token = Arc::new(CancellationToken::new());
        }
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
            sessions.get(id).map(|entry| Arc::clone(&entry.executor))
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
        self.sessions.write().await.remove(id);
    }

    /// Remove a session only if it is currently idle.
    ///
    /// Returns `true` when the session entry was removed, `false` otherwise.
    pub async fn remove_if_idle(&self, id: &SessionId) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(executor_arc) = sessions.get(id).map(|entry| Arc::clone(&entry.executor)) else {
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
        true
    }

    /// Clear all todos for a session
    pub async fn clear_todos(&self, id: &SessionId) -> bool {
        let executor_arc = {
            let sessions = self.sessions.read().await;
            sessions.get(id).map(|entry| Arc::clone(&entry.executor))
        };

        let Some(executor_arc) = executor_arc else {
            return false;
        };

        if let Ok(mut executor) = executor_arc.try_write() {
            executor.session_mut().clear_todos();
            true
        } else {
            false
        }
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
        assert!(registry.execution_handles(&session_id).await.is_none());
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
        assert!(registry.execution_handles(&session_id).await.is_some());
    }

    #[tokio::test]
    async fn cancellation_is_independent_of_executor_lock_and_renewable() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(252_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;
        let (executor, original_token) = registry
            .execution_handles(&session_id)
            .await
            .expect("session entry must exist");

        let _executor_guard = executor.write().await;
        assert!(registry.cancel(&session_id).await);
        assert!(original_token.is_cancelled());

        registry.renew_cancellation_token(&session_id).await;
        let (_, renewed_token) = registry
            .execution_handles(&session_id)
            .await
            .expect("renewed session entry must exist");
        assert!(!renewed_token.is_cancelled());
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

    #[tokio::test]
    async fn resume_with_user_input_clears_pending_request_and_queues_context() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(404_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;

        registry
            .with_executor_mut(&session_id, |executor| {
                Box::pin(async move {
                    executor.session_mut().set_pending_user_input(
                        oxide_agent_core::agent::PendingUserInput {
                            kind: oxide_agent_core::agent::UserInputKind::Url,
                            prompt: "Send a direct URL".to_string(),
                        },
                    );
                })
            })
            .await
            .expect("session must exist for pending user input test");

        let resumed = registry
            .resume_with_user_input(&session_id, "https://example.com/app.apk".to_string())
            .await
            .expect("resume should succeed for idle session");

        assert!(resumed);

        let executor_arc = registry
            .get(&session_id)
            .await
            .expect("session must exist after resume");
        let mut executor = executor_arc.write().await;
        assert!(executor.session().pending_user_input().is_none());
        let pending = executor.session_mut().drain_runtime_context();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "https://example.com/app.apk");
    }

    #[tokio::test]
    async fn resume_with_user_input_returns_false_without_pending_request() {
        let registry = SessionRegistry::new();
        let session_id = SessionId::from(505_i64);
        registry
            .insert(session_id, build_executor(session_id))
            .await;

        let resumed = registry
            .resume_with_user_input(&session_id, "hello".to_string())
            .await
            .expect("resume should succeed for idle session");

        assert!(!resumed);
    }
}
