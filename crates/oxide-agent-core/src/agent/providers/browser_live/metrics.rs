//! Browser Live MVP metrics and redacted structured logging helpers.
//!
//! The collector keeps cheap per-session counters in atomics so that metrics
//! can be updated from async tool code without contending on a shared mutex.
//! Snapshots are serializable and can be returned in tool payloads or emitted as
//! progress events. No raw screenshot bytes, URLs, or typed values are ever
//! stored in metrics.

use crate::llm::TokenUsage;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Snapshot of browser session and MiMo/sidecar metrics.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct BrowserMetricsSnapshot {
    /// Browser sessions started.
    pub sessions_started: u64,
    /// Browser sessions closed.
    pub sessions_closed: u64,
    /// Decisions planned by MiMo (one per browser_step).
    pub actions_planned: u64,
    /// Executable actions dispatched to the sidecar.
    pub actions_executed: u64,
    /// Observations fetched from the sidecar.
    pub observations_fetched: u64,
    /// Screenshots captured/retained.
    pub screenshots_captured: u64,
    /// MiMo decision requests issued.
    pub mimo_requests: u64,
    /// MiMo decision requests that failed.
    pub mimo_errors: u64,
    /// MiMo parse failures that triggered a repair retry.
    pub mimo_repair_attempts: u64,
    /// Invalid JSON outputs seen by the parser (includes repair failures).
    pub mimo_invalid_json: u64,
    /// Recovery attempts executed after a verification failure.
    pub recovery_attempts: u64,
    /// Recovery paths that ended in a safe stop.
    pub recovery_safe_stops: u64,
    /// Sidecar REST requests issued.
    pub sidecar_requests: u64,
    /// Sidecar REST requests that failed.
    pub sidecar_errors: u64,
    /// Total artifact byte volume retained across all sessions.
    pub artifact_bytes_total: u64,
    /// Sum of MiMo prompt tokens reported by the provider.
    pub mimo_prompt_tokens: u64,
    /// Sum of MiMo completion tokens reported by the provider.
    pub mimo_completion_tokens: u64,
    /// Sum of MiMo cached tokens reported by the provider.
    pub mimo_cached_tokens: u64,
    /// Sum of MiMo cache-creation tokens reported by the provider.
    pub mimo_cache_creation_tokens: u64,
    /// Total MiMo decision latency in milliseconds.
    pub mimo_latency_ms_total: u64,
    /// Total sidecar request latency in milliseconds.
    pub sidecar_latency_ms_total: u64,
}

impl BrowserMetricsSnapshot {
    /// Average MiMo request latency in milliseconds, or zero if no requests.
    #[must_use]
    pub fn mimo_latency_ms_avg(&self) -> u64 {
        if self.mimo_requests == 0 {
            0
        } else {
            self.mimo_latency_ms_total / self.mimo_requests
        }
    }

    /// Average sidecar request latency in milliseconds, or zero if no requests.
    #[must_use]
    pub fn sidecar_latency_ms_avg(&self) -> u64 {
        if self.sidecar_requests == 0 {
            0
        } else {
            self.sidecar_latency_ms_total / self.sidecar_requests
        }
    }
}

/// Thread-safe metrics collector for Browser Live.
#[derive(Debug, Default)]
pub struct BrowserMetricsCollector {
    sessions_started: AtomicU64,
    sessions_closed: AtomicU64,
    actions_planned: AtomicU64,
    actions_executed: AtomicU64,
    observations_fetched: AtomicU64,
    screenshots_captured: AtomicU64,
    mimo_requests: AtomicU64,
    mimo_errors: AtomicU64,
    mimo_repair_attempts: AtomicU64,
    mimo_invalid_json: AtomicU64,
    recovery_attempts: AtomicU64,
    recovery_safe_stops: AtomicU64,
    sidecar_requests: AtomicU64,
    sidecar_errors: AtomicU64,
    artifact_bytes_total: AtomicU64,
    mimo_prompt_tokens: AtomicU64,
    mimo_completion_tokens: AtomicU64,
    mimo_cached_tokens: AtomicU64,
    mimo_cache_creation_tokens: AtomicU64,
    mimo_latency_ms_total: AtomicU64,
    sidecar_latency_ms_total: AtomicU64,
}

impl BrowserMetricsCollector {
    /// Create a new empty collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a serializable snapshot of current counters.
    #[must_use]
    pub fn snapshot(&self) -> BrowserMetricsSnapshot {
        BrowserMetricsSnapshot {
            sessions_started: self.sessions_started.load(Ordering::Relaxed),
            sessions_closed: self.sessions_closed.load(Ordering::Relaxed),
            actions_planned: self.actions_planned.load(Ordering::Relaxed),
            actions_executed: self.actions_executed.load(Ordering::Relaxed),
            observations_fetched: self.observations_fetched.load(Ordering::Relaxed),
            screenshots_captured: self.screenshots_captured.load(Ordering::Relaxed),
            mimo_requests: self.mimo_requests.load(Ordering::Relaxed),
            mimo_errors: self.mimo_errors.load(Ordering::Relaxed),
            mimo_repair_attempts: self.mimo_repair_attempts.load(Ordering::Relaxed),
            mimo_invalid_json: self.mimo_invalid_json.load(Ordering::Relaxed),
            recovery_attempts: self.recovery_attempts.load(Ordering::Relaxed),
            recovery_safe_stops: self.recovery_safe_stops.load(Ordering::Relaxed),
            sidecar_requests: self.sidecar_requests.load(Ordering::Relaxed),
            sidecar_errors: self.sidecar_errors.load(Ordering::Relaxed),
            artifact_bytes_total: self.artifact_bytes_total.load(Ordering::Relaxed),
            mimo_prompt_tokens: self.mimo_prompt_tokens.load(Ordering::Relaxed),
            mimo_completion_tokens: self.mimo_completion_tokens.load(Ordering::Relaxed),
            mimo_cached_tokens: self.mimo_cached_tokens.load(Ordering::Relaxed),
            mimo_cache_creation_tokens: self.mimo_cache_creation_tokens.load(Ordering::Relaxed),
            mimo_latency_ms_total: self.mimo_latency_ms_total.load(Ordering::Relaxed),
            sidecar_latency_ms_total: self.sidecar_latency_ms_total.load(Ordering::Relaxed),
        }
    }

    /// Increment the sessions-started counter.
    pub fn record_session_start(&self) {
        self.sessions_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the sessions-closed counter.
    pub fn record_session_close(&self) {
        self.sessions_closed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that one browser step planned a MiMo decision.
    pub fn record_action_planned(&self) {
        self.actions_planned.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that one executable action was dispatched to the sidecar.
    pub fn record_action_executed(&self) {
        self.actions_executed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that one observation was fetched from the sidecar.
    pub fn record_observation_fetched(&self) {
        self.observations_fetched.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that one screenshot artifact was captured/retained.
    pub fn record_screenshot_captured(&self, bytes: u64) {
        self.screenshots_captured.fetch_add(1, Ordering::Relaxed);
        self.artifact_bytes_total
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record one MiMo request, its latency, and optional token usage.
    pub fn record_mimo_request(&self, latency: std::time::Duration, usage: Option<&TokenUsage>) {
        self.mimo_requests.fetch_add(1, Ordering::Relaxed);
        self.mimo_latency_ms_total
            .fetch_add(latency.as_millis() as u64, Ordering::Relaxed);
        self.record_mimo_usage(usage);
    }

    /// Add token counts from a repair or retry response without incrementing the request counter.
    pub fn record_mimo_usage(&self, usage: Option<&TokenUsage>) {
        if let Some(usage) = usage {
            self.mimo_prompt_tokens
                .fetch_add(usage.prompt_tokens as u64, Ordering::Relaxed);
            self.mimo_completion_tokens
                .fetch_add(usage.completion_tokens as u64, Ordering::Relaxed);
            if let Some(cached) = usage.cached_tokens {
                self.mimo_cached_tokens
                    .fetch_add(cached as u64, Ordering::Relaxed);
            }
            if let Some(creation) = usage.cache_creation_tokens {
                self.mimo_cache_creation_tokens
                    .fetch_add(creation as u64, Ordering::Relaxed);
            }
        }
    }

    /// Record a MiMo request failure.
    pub fn record_mimo_error(&self) {
        self.mimo_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that the parser had to perform a repair retry.
    pub fn record_mimo_repair_attempt(&self) {
        self.mimo_repair_attempts.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that the parser saw an invalid JSON output.
    pub fn record_mimo_invalid_json(&self) {
        self.mimo_invalid_json.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one recovery attempt after a verification failure.
    pub fn record_recovery_attempt(&self) {
        self.recovery_attempts.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one recovery path that ended in a safe stop.
    pub fn record_recovery_safe_stop(&self) {
        self.recovery_safe_stops.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one sidecar request and its latency.
    pub fn record_sidecar_request(&self, latency: std::time::Duration) {
        self.sidecar_requests.fetch_add(1, Ordering::Relaxed);
        self.sidecar_latency_ms_total
            .fetch_add(latency.as_millis() as u64, Ordering::Relaxed);
    }

    /// Record one sidecar request failure.
    pub fn record_sidecar_error(&self) {
        self.sidecar_errors.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::TokenUsage;

    #[test]
    fn collector_tracks_session_and_action_counters() {
        let collector = BrowserMetricsCollector::new();
        collector.record_session_start();
        collector.record_action_planned();
        collector.record_action_executed();
        collector.record_observation_fetched();
        collector.record_screenshot_captured(1234);
        collector.record_session_close();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.sessions_started, 1);
        assert_eq!(snapshot.sessions_closed, 1);
        assert_eq!(snapshot.actions_planned, 1);
        assert_eq!(snapshot.actions_executed, 1);
        assert_eq!(snapshot.observations_fetched, 1);
        assert_eq!(snapshot.screenshots_captured, 1);
        assert_eq!(snapshot.artifact_bytes_total, 1234);
    }

    #[test]
    fn collector_tracks_mimo_and_recovery_counters() {
        let collector = BrowserMetricsCollector::new();
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cached_tokens: Some(25),
            cache_creation_tokens: Some(10),
        };
        collector.record_mimo_request(std::time::Duration::from_millis(120), Some(&usage));
        collector.record_mimo_error();
        collector.record_mimo_repair_attempt();
        collector.record_mimo_invalid_json();
        collector.record_recovery_attempt();
        collector.record_recovery_safe_stop();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.mimo_requests, 1);
        assert_eq!(snapshot.mimo_latency_ms_total, 120);
        assert_eq!(snapshot.mimo_latency_ms_avg(), 120);
        assert_eq!(snapshot.mimo_prompt_tokens, 100);
        assert_eq!(snapshot.mimo_completion_tokens, 50);
        assert_eq!(snapshot.mimo_cached_tokens, 25);
        assert_eq!(snapshot.mimo_cache_creation_tokens, 10);
        assert_eq!(snapshot.mimo_errors, 1);
        assert_eq!(snapshot.mimo_repair_attempts, 1);
        assert_eq!(snapshot.mimo_invalid_json, 1);
        assert_eq!(snapshot.recovery_attempts, 1);
        assert_eq!(snapshot.recovery_safe_stops, 1);
    }

    #[test]
    fn collector_tracks_sidecar_latency_and_errors() {
        let collector = BrowserMetricsCollector::new();
        collector.record_sidecar_request(std::time::Duration::from_millis(45));
        collector.record_sidecar_request(std::time::Duration::from_millis(55));
        collector.record_sidecar_error();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.sidecar_requests, 2);
        assert_eq!(snapshot.sidecar_latency_ms_total, 100);
        assert_eq!(snapshot.sidecar_latency_ms_avg(), 50);
        assert_eq!(snapshot.sidecar_errors, 1);
    }

    #[test]
    fn snapshot_serialization_excludes_sensitive_bytes_and_urls() {
        let collector = BrowserMetricsCollector::new();
        collector.record_screenshot_captured(4096);
        let json = serde_json::to_string(&collector.snapshot()).expect("serialize");
        assert!(!json.contains("base64"));
        assert!(!json.contains("data:image"));
        assert!(!json.contains("http://"));
        assert!(!json.contains("https://"));
        assert!(json.contains("screenshots_captured"));
    }
}
