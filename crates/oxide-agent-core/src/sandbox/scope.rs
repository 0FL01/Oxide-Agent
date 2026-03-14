//! Stable sandbox identity for persistent Docker containers.

use std::collections::HashMap;

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Stable sandbox scope used to derive persistent container identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxScope {
    owner_id: i64,
    namespace: String,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
}

impl SandboxScope {
    /// Create a new sandbox scope.
    #[must_use]
    pub fn new(owner_id: i64, namespace: impl Into<String>) -> Self {
        Self {
            owner_id,
            namespace: namespace.into(),
            chat_id: None,
            thread_id: None,
        }
    }

    /// Attach optional transport metadata for diagnostics.
    #[must_use]
    pub fn with_transport_metadata(mut self, chat_id: Option<i64>, thread_id: Option<i64>) -> Self {
        self.chat_id = chat_id;
        self.thread_id = thread_id;
        self
    }

    /// Owning user/session identifier.
    #[must_use]
    pub const fn owner_id(&self) -> i64 {
        self.owner_id
    }

    /// Logical namespace for this sandbox.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Optional transport chat identifier.
    #[must_use]
    pub const fn chat_id(&self) -> Option<i64> {
        self.chat_id
    }

    /// Optional transport thread/topic identifier.
    #[must_use]
    pub const fn thread_id(&self) -> Option<i64> {
        self.thread_id
    }

    /// Deterministic Docker container name for this scope.
    #[must_use]
    pub fn container_name(&self) -> String {
        format!(
            "agent-sandbox-u{}-{:016x}",
            self.owner_id,
            self.namespace_hash()
        )
    }

    /// Docker labels for diagnostics and cleanup.
    #[must_use]
    pub fn docker_labels(&self) -> HashMap<String, String> {
        let mut labels = HashMap::from([
            ("agent.user_id".to_string(), self.owner_id.to_string()),
            ("agent.sandbox".to_string(), "true".to_string()),
            ("agent.scope".to_string(), self.namespace.clone()),
        ]);

        if let Some(chat_id) = self.chat_id {
            labels.insert("agent.chat_id".to_string(), chat_id.to_string());
        }

        if let Some(thread_id) = self.thread_id {
            labels.insert("agent.thread_id".to_string(), thread_id.to_string());
        }

        labels
    }

    fn namespace_hash(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        for byte in self.namespace.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }
}

impl From<i64> for SandboxScope {
    fn from(value: i64) -> Self {
        Self::new(value, format!("default:{value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::SandboxScope;

    #[test]
    fn container_name_is_stable_for_same_scope() {
        let first = SandboxScope::new(77, "-100123:69").container_name();
        let second = SandboxScope::new(77, "-100123:69").container_name();

        assert_eq!(first, second);
    }

    #[test]
    fn container_name_differs_for_different_namespaces() {
        let general = SandboxScope::new(77, "-100123:1").container_name();
        let alfa = SandboxScope::new(77, "-100123:69").container_name();

        assert_ne!(general, alfa);
    }

    #[test]
    fn docker_labels_include_transport_metadata() {
        let labels = SandboxScope::new(77, "-100123:69")
            .with_transport_metadata(Some(-100123), Some(69))
            .docker_labels();

        assert_eq!(labels.get("agent.user_id"), Some(&"77".to_string()));
        assert_eq!(labels.get("agent.scope"), Some(&"-100123:69".to_string()));
        assert_eq!(labels.get("agent.chat_id"), Some(&"-100123".to_string()));
        assert_eq!(labels.get("agent.thread_id"), Some(&"69".to_string()));
    }
}
