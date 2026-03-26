use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{info, trace};

tokio::task_local! {
    static STORAGE_REASON: &'static str;
}

const SUMMARY_EVERY_EVENTS: u64 = 128;

#[derive(Clone, Copy, Debug)]
pub(crate) enum StorageOperation {
    Get,
    Put,
    List,
    Delete,
}

impl StorageOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Put => "put",
            Self::List => "list",
            Self::Delete => "delete",
        }
    }

    const fn class(self) -> &'static str {
        match self {
            Self::Get => "class_b",
            Self::Put | Self::List | Self::Delete => "class_a",
        }
    }
}

#[derive(Default)]
pub(crate) struct StorageTelemetry {
    get_ops: AtomicU64,
    put_ops: AtomicU64,
    list_ops: AtomicU64,
    delete_ops: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    observed_events: AtomicU64,
}

impl StorageTelemetry {
    pub(crate) fn record_operation(
        &self,
        operation: StorageOperation,
        path: &str,
        outcome: &'static str,
    ) {
        let total = match operation {
            StorageOperation::Get => self.get_ops.fetch_add(1, Ordering::Relaxed) + 1,
            StorageOperation::Put => self.put_ops.fetch_add(1, Ordering::Relaxed) + 1,
            StorageOperation::List => self.list_ops.fetch_add(1, Ordering::Relaxed) + 1,
            StorageOperation::Delete => self.delete_ops.fetch_add(1, Ordering::Relaxed) + 1,
        };
        let reason = current_storage_reason();
        let scope = storage_scope(path);

        trace!(
            storage_op = operation.as_str(),
            storage_class = operation.class(),
            storage_scope = scope,
            storage_reason = reason,
            storage_outcome = outcome,
            storage_total = total,
            storage_path = path,
            "R2 storage operation"
        );

        self.maybe_emit_summary(reason, scope);
    }

    pub(crate) fn record_cache_hit(&self, path: &str) {
        let total = self.cache_hits.fetch_add(1, Ordering::Relaxed) + 1;
        let reason = current_storage_reason();
        let scope = storage_scope(path);

        trace!(
            storage_cache = "hit",
            storage_scope = scope,
            storage_reason = reason,
            storage_total = total,
            storage_path = path,
            "R2 storage cache hit"
        );

        self.maybe_emit_summary(reason, scope);
    }

    pub(crate) fn record_cache_miss(&self, path: &str) {
        let total = self.cache_misses.fetch_add(1, Ordering::Relaxed) + 1;
        let reason = current_storage_reason();
        let scope = storage_scope(path);

        trace!(
            storage_cache = "miss",
            storage_scope = scope,
            storage_reason = reason,
            storage_total = total,
            storage_path = path,
            "R2 storage cache miss"
        );

        self.maybe_emit_summary(reason, scope);
    }

    fn maybe_emit_summary(&self, reason: &'static str, scope: &'static str) {
        let observed = self.observed_events.fetch_add(1, Ordering::Relaxed) + 1;
        if !observed.is_multiple_of(SUMMARY_EVERY_EVENTS) {
            return;
        }

        info!(
            storage_events = observed,
            storage_get_ops = self.get_ops.load(Ordering::Relaxed),
            storage_put_ops = self.put_ops.load(Ordering::Relaxed),
            storage_list_ops = self.list_ops.load(Ordering::Relaxed),
            storage_delete_ops = self.delete_ops.load(Ordering::Relaxed),
            storage_cache_hits = self.cache_hits.load(Ordering::Relaxed),
            storage_cache_misses = self.cache_misses.load(Ordering::Relaxed),
            last_storage_reason = reason,
            last_storage_scope = scope,
            "R2 storage telemetry summary"
        );
    }
}

pub(crate) async fn with_storage_reason<T, F>(reason: &'static str, future: F) -> T
where
    F: Future<Output = T>,
{
    if STORAGE_REASON.try_with(|_| ()).is_ok() {
        future.await
    } else {
        STORAGE_REASON.scope(reason, future).await
    }
}

pub(crate) fn current_storage_reason() -> &'static str {
    STORAGE_REASON
        .try_with(|reason| *reason)
        .unwrap_or("unspecified")
}

pub(crate) fn storage_scope(path: &str) -> &'static str {
    let normalized = path.trim_end_matches('/');

    if normalized == "users" {
        return "users_root";
    }
    if normalized.ends_with("/config.json") {
        return "user_config";
    }
    if normalized.ends_with("/history.json") {
        if normalized.contains("/chats/") {
            return "chat_history";
        }
        return "legacy_chat_history";
    }
    if normalized.ends_with("/agent_memory.json") {
        return "topic_memory";
    }
    if normalized.ends_with("/memory.json") && normalized.contains("/flows/") {
        return "flow_memory";
    }
    if normalized.ends_with("/meta.json") && normalized.contains("/flows/") {
        return "flow_meta";
    }
    if normalized.contains("/control_plane/agent_profiles/") {
        return "agent_profile";
    }
    if normalized.contains("/control_plane/topic_contexts/") {
        return "topic_context";
    }
    if normalized.contains("/control_plane/topic_agents_md/") {
        return "topic_agents_md";
    }
    if normalized.contains("/control_plane/topic_infra/") {
        return "topic_infra";
    }
    if normalized.contains("/control_plane/topic_bindings/") {
        return "topic_binding";
    }
    if normalized.contains("/control_plane/reminders/") {
        if normalized.ends_with("/control_plane/reminders") {
            return "reminder_prefix";
        }
        return "reminder_job";
    }
    if normalized.contains("/control_plane/audit/") {
        return "audit";
    }
    if normalized.contains("/private/secrets/") {
        return "secret";
    }
    if normalized.contains("/topics/") && normalized.ends_with("/flows") {
        return "flow_prefix";
    }
    if normalized.contains("/topics/") {
        return "topic_prefix";
    }
    if normalized.starts_with("users/") {
        return "user_prefix";
    }

    "other"
}

#[cfg(test)]
mod tests {
    use super::{storage_scope, with_storage_reason};

    #[test]
    fn classifies_storage_scopes() {
        assert_eq!(storage_scope("users/7/config.json"), "user_config");
        assert_eq!(
            storage_scope("users/7/chats/a/history.json"),
            "chat_history"
        );
        assert_eq!(
            storage_scope("users/7/topics/topic/flows/flow-1/memory.json"),
            "flow_memory"
        );
        assert_eq!(
            storage_scope("users/7/control_plane/topic_bindings/topic.json"),
            "topic_binding"
        );
        assert_eq!(
            storage_scope("users/7/control_plane/reminders/reminder-1.json"),
            "reminder_job"
        );
    }

    #[tokio::test]
    async fn preserves_outer_storage_reason() {
        let observed = with_storage_reason("outer", async {
            with_storage_reason("inner", async { super::current_storage_reason() }).await
        })
        .await;

        assert_eq!(observed, "outer");
    }
}
