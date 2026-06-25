//! Browser Live MVP metrics and redacted structured logging helpers.
//!
//! The collector keeps cheap per-session counters in atomics so that metrics
//! can be updated from async tool code without contending on a shared mutex.
//! Snapshots are serializable and can be returned in tool payloads or emitted as
//! progress events. No raw screenshot bytes, URLs, or typed values are ever
//! stored in metrics.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Snapshot of browser session and sidecar metrics.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct BrowserMetricsSnapshot {
    /// Browser sessions started.
    pub sessions_started: u64,
    /// Browser sessions closed.
    pub sessions_closed: u64,
    /// Observations fetched from the sidecar.
    pub observations_fetched: u64,
    /// Screenshots captured/retained.
    pub screenshots_captured: u64,
    /// Sidecar REST requests issued.
    pub sidecar_requests: u64,
    /// Sidecar REST requests that failed.
    pub sidecar_errors: u64,
    /// Total artifact byte volume retained across all sessions.
    pub artifact_bytes_total: u64,
    /// Total sidecar request latency in milliseconds.
    pub sidecar_latency_ms_total: u64,
}

impl BrowserMetricsSnapshot {
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
    observations_fetched: AtomicU64,
    screenshots_captured: AtomicU64,
    sidecar_requests: AtomicU64,
    sidecar_errors: AtomicU64,
    artifact_bytes_total: AtomicU64,
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
            observations_fetched: self.observations_fetched.load(Ordering::Relaxed),
            screenshots_captured: self.screenshots_captured.load(Ordering::Relaxed),
            sidecar_requests: self.sidecar_requests.load(Ordering::Relaxed),
            sidecar_errors: self.sidecar_errors.load(Ordering::Relaxed),
            artifact_bytes_total: self.artifact_bytes_total.load(Ordering::Relaxed),
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

    #[test]
    fn collector_tracks_session_and_observation_counters() {
        let collector = BrowserMetricsCollector::new();
        collector.record_session_start();
        collector.record_observation_fetched();
        collector.record_screenshot_captured(1234);
        collector.record_session_close();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.sessions_started, 1);
        assert_eq!(snapshot.sessions_closed, 1);
        assert_eq!(snapshot.observations_fetched, 1);
        assert_eq!(snapshot.screenshots_captured, 1);
        assert_eq!(snapshot.artifact_bytes_total, 1234);
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
