use crate::agent::compaction::{
    CompactionRequest, CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
};
use crate::llm::LlmClient;
use std::sync::Arc;

pub(super) fn fallback_summarizer_service() -> CompactionService {
    let llm_client = Arc::new(LlmClient::new(&crate::config::AgentSettings::default()));
    CompactionService::default().with_summarizer(CompactionSummarizer::new(
        llm_client,
        CompactionSummarizerConfig {
            model_name: String::new(),
            provider_name: String::new(),
            timeout_secs: 1,
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
