//! Conversation identity for provider routing, isolated between concurrent async tasks.

use std::future::Future;

tokio::task_local! {
    static LLM_SESSION_ID: String;
}

/// Run LLM work with a stable conversation ID for provider routing and prompt caching.
/// Spawned tasks must establish their own scope; the ID is not process-global.
pub async fn with_llm_session<F: Future>(session_id: String, future: F) -> F::Output {
    LLM_SESSION_ID.scope(session_id, future).await
}

/// Standalone requests without a conversation get an independent routing identity.
pub(crate) fn current_session_id() -> String {
    LLM_SESSION_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_identity_is_stable_and_isolated_between_conversations() {
        let check = |id: &'static str| async move {
            with_llm_session(id.to_string(), async {
                assert_eq!(current_session_id(), id);
                tokio::task::yield_now().await;
                assert_eq!(current_session_id(), id);
            })
            .await;
        };
        tokio::join!(check("conversation-a"), check("conversation-b"));
        assert_ne!(current_session_id(), current_session_id());
    }
}
