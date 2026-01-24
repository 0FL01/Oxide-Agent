//! Testing helpers and mock utilities.
//!
//! Provides convenient constructors for mocked LLM and storage providers.

use crate::agent::memory::AgentMemory;
use crate::llm::{LlmError, LlmProvider};
use crate::storage::{Message as StorageMessage, StorageProvider, UserConfig};
use mockall::predicate::*;

/// Create a mock LLM provider that returns a simple text response.
///
/// # Returns
///
/// A `MockLlmProvider` that returns the provided `response_text` for all
/// `chat_completion` calls. Other methods return error by default.
///
/// # Example
///
/// ```rust,ignore
/// use oxide_agent_core::testing::mock_llm_simple;
///
/// let mut mock = mock_llm_simple("Hello, world!");
/// // Use the mock in tests...
/// ```
#[must_use]
pub fn mock_llm_simple(response_text: &'static str) -> crate::llm::MockLlmProvider {
    let mut mock = crate::llm::MockLlmProvider::new();
    mock.expect_chat_completion()
        .with(always(), always(), always(), always(), always())
        .returning(move |_, _, _, _, _| Ok(response_text.to_string()));

    mock.expect_transcribe_audio()
        .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));

    mock.expect_analyze_image()
        .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));

    mock
}

/// Create a mock storage provider that performs no operations (noop).
///
/// All methods return default/empty values without errors, making it suitable
/// for tests that don't require persistent storage.
///
/// # Returns
///
/// A `MockStorageProvider` where:
/// - `get_user_config` returns a default empty `UserConfig`
/// - `update_user_config` / `update_user_prompt` / `update_user_model` / `update_user_state` return `Ok(())`
/// - `get_user_prompt` / `get_user_model` / `get_user_state` return `Ok(None)`
/// - `save_message` returns `Ok(())`
/// - `get_chat_history` returns an empty `Vec<Message>`
/// - `clear_chat_history` / `clear_agent_memory` / `clear_all_context` return `Ok(())`
/// - `save_agent_memory` returns `Ok(())`
/// - `load_agent_memory` returns `Ok(None)`
/// - `check_connection` returns `Ok(())`
///
/// # Example
///
/// ```rust,ignore
/// use oxide_agent_core::testing::mock_storage_noop;
///
/// let storage = mock_storage_noop();
/// // Use the mock in tests that don't need storage...
/// ```
#[must_use]
pub fn mock_storage_noop() -> crate::storage::MockStorageProvider {
    let mut mock = crate::storage::MockStorageProvider::new();

    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));

    mock.expect_update_user_config().returning(|_, _| Ok(()));

    mock.expect_update_user_prompt().returning(|_, _| Ok(()));

    mock.expect_get_user_prompt().returning(|_| Ok(None));

    mock.expect_update_user_model().returning(|_, _| Ok(()));

    mock.expect_get_user_model().returning(|_| Ok(None));

    mock.expect_update_user_state().returning(|_, _| Ok(()));

    mock.expect_get_user_state().returning(|_| Ok(None));

    mock.expect_save_message().returning(|_, _, _| Ok(()));

    mock.expect_get_chat_history()
        .returning(|_, _| Ok(Vec::new()));

    mock.expect_clear_chat_history().returning(|_| Ok(()));

    mock.expect_save_agent_memory().returning(|_, _| Ok(()));

    mock.expect_load_agent_memory().returning(|_| Ok(None));

    mock.expect_clear_agent_memory().returning(|_| Ok(()));

    mock.expect_clear_all_context().returning(|_| Ok(()));

    mock.expect_check_connection().returning(|| Ok(()));

    mock
}
