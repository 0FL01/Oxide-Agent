use crate::{config::ModelSelection, tasks::TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default title assigned when a session is first created.
const WEB_SESSION_DEFAULT_TITLE: &str = "New session";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebSessionRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub user_id: i64,
    pub title: String,
    pub context_key: String,
    #[serde(default)]
    pub context_keys: Vec<String>,
    pub agent_flow_id: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub active_task_id: Option<String>,
    pub last_task_status: Option<TaskStatus>,
    pub last_preview: Option<String>,
    pub manually_renamed: bool,
    #[serde(default)]
    pub auto_title_source_message: Option<String>,
    #[serde(default)]
    pub auto_title_replaceable_title: Option<String>,
    #[serde(default)]
    pub auto_title_attempts: u32,
    #[serde(default)]
    pub auto_title_next_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub auto_title_last_error: Option<String>,
}

impl WebSessionRecord {
    /// Returns `true` when the async auto-title worker may overwrite the
    /// current title.  The title is considered replaceable if the user has
    /// not manually renamed the session **and** the title is still the
    /// default or the fallback preview that was set on first task creation.
    pub fn may_auto_title(&self, fallback_preview: &str) -> bool {
        !self.manually_renamed
            && (self.title == WEB_SESSION_DEFAULT_TITLE
                || self.title == fallback_preview
                || looks_like_timestamp_title(&self.title))
    }

    pub fn tracked_context_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for key in self
            .context_keys
            .iter()
            .chain(std::iter::once(&self.context_key))
        {
            if !key.is_empty() && !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        keys
    }
}

fn looks_like_timestamp_title(value: &str) -> bool {
    let value = value.trim();
    let bytes = value.as_bytes();
    bytes.len() >= 16
        && bytes
            .get(0..4)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(4) == Some(&b'-')
        && bytes
            .get(5..7)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(7) == Some(&b'-')
        && bytes
            .get(8..10)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && matches!(bytes.get(10), Some(b' ' | b'T'))
        && bytes
            .get(11..13)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(13) == Some(&b':')
        && bytes
            .get(14..16)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    pub last_preview: Option<String>,
    pub active_task_id: Option<String>,
    pub last_task_status: Option<TaskStatus>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionDetail {
    pub session_id: String,
    pub title: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    pub last_preview: Option<String>,
    pub active_task_id: Option<String>,
    pub last_task_status: Option<TaskStatus>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub agent_profile_selection: AgentProfileSelection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentProfileSelection {
    #[default]
    Default,
    None,
    Profile {
        agent_profile_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateSessionResponse {
    pub session: SessionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GetSessionResponse {
    pub session: SessionDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateSessionRequest {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateSessionProfileRequest {
    #[serde(default)]
    pub agent_profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateSessionResponse {
    pub session: SessionDetail,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_title(title: &str) -> WebSessionRecord {
        let now = Utc::now();
        WebSessionRecord {
            schema_version: 1,
            session_id: "session".to_string(),
            user_id: 7,
            title: title.to_string(),
            context_key: "context".to_string(),
            context_keys: vec!["context".to_string()],
            agent_flow_id: "main".to_string(),
            model_selection: None,
            agent_profile_id: None,
            created_at: now,
            updated_at: now,
            active_task_id: None,
            last_task_status: None,
            last_preview: None,
            manually_renamed: false,
            auto_title_source_message: None,
            auto_title_replaceable_title: None,
            auto_title_attempts: 0,
            auto_title_next_attempt_at: None,
            auto_title_last_error: None,
        }
    }

    #[test]
    fn auto_title_may_replace_timestamp_titles() {
        let session = session_with_title("2026-05-29 20:53:47.208618014");

        assert!(session.may_auto_title("fallback"));
    }

    #[test]
    fn auto_title_does_not_replace_manual_timestamp_titles() {
        let mut session = session_with_title("2026-05-29 20:53:47.208618014");
        session.manually_renamed = true;

        assert!(!session.may_auto_title("fallback"));
    }
}
