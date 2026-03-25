use crate::agent::compaction::{
    ArchiveChunk, ArchiveRef, ArchiveSink, CompactionPolicy, CompactionRequest, CompactionService,
    CompactionSummarizer, CompactionSummarizerConfig, ExternalizedPayloadRecord, PayloadSink,
};
use crate::llm::LlmClient;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::sync::Mutex;

pub(super) fn fallback_summarizer_service() -> CompactionService {
    let llm_client = Arc::new(LlmClient::new(&crate::config::AgentSettings::default()));
    CompactionService::default().with_summarizer(CompactionSummarizer::new(
        llm_client,
        CompactionSummarizerConfig {
            model_routes: Vec::new(),
            timeout_secs: 1,
            ..CompactionSummarizerConfig::default()
        },
    ))
}

pub(super) fn manual_request(task: &'static str) -> CompactionRequest<'static> {
    CompactionRequest::new(
        crate::agent::compaction::CompactionTrigger::Manual,
        task,
        "system prompt",
        &[],
        "demo-model",
        256,
        false,
    )
}

pub(super) fn pre_iteration_request(task: &'static str) -> CompactionRequest<'static> {
    CompactionRequest::new(
        crate::agent::compaction::CompactionTrigger::PreIteration,
        task,
        "system prompt",
        &[],
        "demo-model",
        256,
        false,
    )
}

pub(super) fn cleanup_policy() -> CompactionPolicy {
    CompactionPolicy {
        externalize_threshold_tokens: usize::MAX,
        externalize_threshold_chars: 16,
        externalize_preview_chars: 24,
        prune_min_tokens: usize::MAX,
        prune_min_chars: 50,
        prune_preview_chars: 20,
        protected_tool_window_tokens: 1,
        ..CompactionPolicy::default()
    }
}

#[derive(Debug)]
pub(super) struct RecordingPayloadSink {
    records: Mutex<Vec<ExternalizedPayloadRecord>>,
    persisted: bool,
}

impl RecordingPayloadSink {
    pub(super) fn new(persisted: bool) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            persisted,
        }
    }

    pub(super) fn records(&self) -> Vec<ExternalizedPayloadRecord> {
        self.records.lock().expect("payload records lock").clone()
    }
}

impl PayloadSink for RecordingPayloadSink {
    fn persist(&self, record: &ExternalizedPayloadRecord) -> Result<bool> {
        self.records
            .lock()
            .expect("payload records lock")
            .push(record.clone());
        Ok(self.persisted)
    }
}

#[derive(Debug, Default)]
pub(super) struct RecordingArchiveSink {
    records: Mutex<Vec<ArchiveChunk>>,
}

impl RecordingArchiveSink {
    pub(super) fn records(&self) -> Vec<ArchiveChunk> {
        self.records.lock().expect("archive records lock").clone()
    }
}

impl ArchiveSink for RecordingArchiveSink {
    fn persist(&self, chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>> {
        self.records
            .lock()
            .expect("archive records lock")
            .push(chunk.clone());
        Ok(Some(ArchiveRef {
            archive_id: chunk.record.archive_id.clone(),
            created_at: chunk.record.created_at,
            title: chunk.record.title.clone(),
            storage_key: chunk.record.payload_ref.clone(),
        }))
    }
}

#[derive(Debug, Default)]
pub(super) struct ErrorPayloadSink;

impl PayloadSink for ErrorPayloadSink {
    fn persist(&self, _record: &ExternalizedPayloadRecord) -> Result<bool> {
        Err(anyhow!("synthetic payload sink failure"))
    }
}
