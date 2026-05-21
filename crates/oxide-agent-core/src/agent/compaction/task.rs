//! Compact task backend contracts.

use super::PreviousCompactedSummary;
use crate::agent::memory::AgentMessage;
use crate::config::ModelInfo;
use async_trait::async_trait;
use thiserror::Error;

/// Provider-neutral input for generating one compact handoff summary.
#[derive(Debug, Clone, Copy)]
pub struct CompactSummaryRequest<'a> {
    /// User-visible task/current objective.
    pub task: &'a str,
    /// Source hot-memory messages to summarize.
    pub messages: &'a [AgentMessage],
    /// Previous compacted or legacy summary, if one was detected.
    pub previous_summary: Option<&'a PreviousCompactedSummary>,
}

/// Plain text summary returned by a compact backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummaryResult {
    /// Trimmed plain text handoff summary.
    pub summary_text: String,
    /// Provider used for the summary request.
    pub provider: String,
    /// Model/route used for the summary request.
    pub route: String,
}

/// Summary backend failure.
#[derive(Debug, Error)]
pub enum CompactSummaryError {
    /// No usable model route is available.
    #[error("no compaction summary route is configured")]
    NoRoute,
    /// Backend returned empty text.
    #[error("compaction summary output is empty")]
    EmptyOutput,
    /// Backend timed out.
    #[error("compaction summary request timed out after {timeout_secs}s")]
    Timeout {
        /// Timeout in seconds.
        timeout_secs: u64,
    },
    /// Provider request failed.
    #[error("compaction summary provider request failed: {0}")]
    Provider(String),
}

/// Backend capable of producing a plain text compact handoff summary.
#[async_trait]
pub trait CompactSummaryBackend: Send + Sync {
    /// Generate one compact handoff summary.
    async fn summarize(
        &self,
        request: CompactSummaryRequest<'_>,
    ) -> Result<CompactSummaryResult, CompactSummaryError>;

    /// Route selected for the next summary request, if available.
    fn selected_route(&self) -> Option<&ModelInfo>;
}
