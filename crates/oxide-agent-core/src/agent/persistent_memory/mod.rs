use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use crate::config::ModelInfo;
use crate::llm::{EmbeddingTaskType, LlmClient, LlmError};
use crate::storage::{StorageMemoryRepository, StorageProvider};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lazy_regex::lazy_regex;
use oxide_agent_memory::{
    stable_memory_content_hash, ArtifactRef, ConsolidationPolicy, ContextConsolidator,
    EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType, EmbeddingPendingUpdate,
    EmbeddingReadyUpdate, EmbeddingRecord, EmbeddingUpdateBase, EpisodeEmbeddingCandidate,
    EpisodeFinalizationInput, EpisodeFinalizer, EpisodeListFilter, EpisodeOutcome, EpisodeRecord,
    EpisodeSearchFilter, EpisodeSearchHit, MemoryEmbeddingCandidate, MemoryListFilter,
    MemoryRecord, MemoryRepository, MemorySearchFilter, MemorySearchHit, MemoryType,
    RepositoryError, SessionStateListFilter, SessionStateRecord, ThreadRecord, TimeRange,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

mod behavior;
mod coordinator;
mod embeddings;
mod post_run;
mod retrieval;
mod store;

#[cfg(test)]
mod tests;

pub use behavior::{MemoryBehaviorRuntime, ToolDerivedMemoryDraft, TopicMemoryPolicy};
pub use coordinator::PersistentMemoryCoordinator;
pub use embeddings::{
    LlmMemoryEmbeddingGenerator, MemoryEmbeddingGenerator, PersistentMemoryEmbeddingIndexer,
};
pub use retrieval::{DurableMemoryRetrievalOptions, DurableMemoryRetriever};
pub use store::{connect_postgres_memory_store, PersistentMemoryStore};

pub(crate) use post_run::{
    LlmPostRunMemoryWriter, PersistentRunContext, PersistentRunPhase, PostRunMemoryWriter,
    PostRunMemoryWriterConfig, PostRunMemoryWriterInput,
};
pub(crate) use retrieval::{
    DurableMemoryRetrievalDiagnostics, DurableMemorySearchItem, DurableMemorySearchOutcome,
    DurableMemorySearchRequest,
};

#[cfg(test)]
pub(crate) use post_run::{ValidatedPostRunEpisode, ValidatedPostRunMemoryWrite};

#[cfg(test)]
pub(crate) use retrieval::HybridCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryTaskClass {
    Smalltalk,
    EpisodeHistory,
    ExternalFreshFact,
    ProcedureHowTo,
    ConstraintPolicy,
    PreferenceRecall,
    DecisionRecall,
    DurableProjectFact,
    General,
}

impl MemoryTaskClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Smalltalk => "smalltalk",
            Self::EpisodeHistory => "episode_history",
            Self::ExternalFreshFact => "external_fresh_fact",
            Self::ProcedureHowTo => "procedure_howto",
            Self::ConstraintPolicy => "constraint_policy",
            Self::PreferenceRecall => "preference_recall",
            Self::DecisionRecall => "decision_recall",
            Self::DurableProjectFact => "durable_project_fact",
            Self::General => "general",
        }
    }

    const fn allows_llm_durable_writes(self) -> bool {
        matches!(
            self,
            Self::ProcedureHowTo
                | Self::ConstraintPolicy
                | Self::PreferenceRecall
                | Self::DecisionRecall
                | Self::DurableProjectFact
        )
    }

    const fn allows_vector_only_memory(self) -> bool {
        matches!(
            self,
            Self::ProcedureHowTo
                | Self::ConstraintPolicy
                | Self::PreferenceRecall
                | Self::DecisionRecall
                | Self::DurableProjectFact
        )
    }
}

pub(crate) fn classify_memory_task(task: &str) -> MemoryTaskClass {
    let normalized = task.trim().to_lowercase();
    if normalized.is_empty() {
        return MemoryTaskClass::General;
    }

    if is_smalltalk_task(&normalized) {
        return MemoryTaskClass::Smalltalk;
    }

    if has_history_cue(&normalized) {
        return MemoryTaskClass::EpisodeHistory;
    }

    let has_project_cue = has_project_cue(&normalized);
    if has_constraint_cue(&normalized) {
        return MemoryTaskClass::ConstraintPolicy;
    }

    if has_preference_cue(&normalized) {
        return MemoryTaskClass::PreferenceRecall;
    }

    if has_decision_cue(&normalized) {
        return MemoryTaskClass::DecisionRecall;
    }

    if has_procedure_cue(&normalized) {
        return MemoryTaskClass::ProcedureHowTo;
    }

    if has_freshness_cue(&normalized) && !has_project_cue {
        return MemoryTaskClass::ExternalFreshFact;
    }

    if has_project_cue {
        return MemoryTaskClass::DurableProjectFact;
    }

    MemoryTaskClass::General
}

pub(crate) fn allow_llm_durable_memory_writes(
    task_class: MemoryTaskClass,
    explicit_remember_intent: bool,
) -> bool {
    explicit_remember_intent || task_class.allows_llm_durable_writes()
}

fn contains_task_phrase(normalized: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| normalized.contains(phrase))
}

fn is_smalltalk_task(normalized: &str) -> bool {
    let token_count = normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .count();
    token_count <= 6
        && SMALLTALK_TASK_PHRASES
            .iter()
            .any(|phrase| normalized == *phrase || normalized.starts_with(&format!("{phrase} ")))
}

fn has_history_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, HISTORY_TASK_CUES)
}

fn has_project_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, PROJECT_TASK_CUES)
}

fn has_constraint_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, CONSTRAINT_TASK_CUES)
}

fn has_preference_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, PREFERENCE_TASK_CUES)
}

fn has_decision_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, DECISION_TASK_CUES)
}

fn has_procedure_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, PROCEDURE_TASK_CUES)
}

fn has_freshness_cue(normalized: &str) -> bool {
    contains_task_phrase(normalized, FRESHNESS_TASK_CUES)
}

const SMALLTALK_TASK_PHRASES: &[&str] = &[
    "thanks",
    "thank you",
    "hello",
    "hi",
    "ok",
    "okay",
    "got it",
    "sounds good",
    "спасибо",
    "спс",
    "ок",
    "окей",
    "понял",
    "понятно",
    "привет",
    "хорошо",
];

const HISTORY_TASK_CUES: &[&str] = &[
    "previous",
    "earlier",
    "before",
    "again",
    "history",
    "thread",
    "episode",
    "what happened",
    "why did",
    "why was",
    "incident",
    "regression",
    "error",
    "issue",
    "debug",
    "resolved",
    "past chat",
    "прошл",
    "раньше",
    "истори",
    "тред",
    "чат",
    "эпизод",
    "что было",
    "что спрашивал",
    "о чем мы говорили",
    "почему",
    "ошибк",
    "дебаг",
    "что делали",
];

const PROJECT_TASK_CUES: &[&str] = &[
    "project",
    "repo",
    "repository",
    "crate",
    "module",
    "config",
    "env",
    "environment variable",
    "setting",
    "feature",
    "codebase",
    "agent",
    "memory",
    "tool",
    "sandbox",
    "browser_use",
    "build",
    "cargo",
    "test",
    "deploy",
    "topic",
    "flow",
    "prompt",
    "workspace",
    "file",
    "server",
    "rust",
    "telegram",
    "проект",
    "репо",
    "репозитор",
    "crate",
    "модул",
    "конфиг",
    "перемен",
    "env",
    "настрой",
    "фича",
    "код",
    "кодовая база",
    "агент",
    "памят",
    "инструмент",
    "sandbox",
    "browser_use",
    "сборк",
    "cargo",
    "тест",
    "депло",
    "топик",
    "flow",
    "промпт",
    "workspace",
    "файл",
    "сервер",
    "rust",
    "telegram",
];

const CONSTRAINT_TASK_CUES: &[&str] = &[
    "constraint",
    "must",
    "never",
    "required",
    "policy",
    "guardrail",
    "forbid",
    "allowed",
    "prohibited",
    "огранич",
    "нельзя",
    "нужно",
    "обязан",
    "требу",
    "политик",
    "запрещ",
    "правил",
    "разреш",
];

const PREFERENCE_TASK_CUES: &[&str] = &[
    "prefer",
    "preference",
    "style",
    "guideline",
    "convention",
    "format",
    "предпоч",
    "стиль",
    "гайд",
    "конвенц",
    "формат",
];

const DECISION_TASK_CUES: &[&str] = &[
    "decision",
    "decided",
    "chosen",
    "решен",
    "решили",
    "выбра",
    "договор",
];

const PROCEDURE_TASK_CUES: &[&str] = &[
    "how to",
    "steps",
    "procedure",
    "workflow",
    "run ",
    "deploy",
    "setup",
    "configure",
    "install",
    "fix",
    "implement",
    "update",
    "edit",
    "write file",
    "шаг",
    "процедур",
    "workflow",
    "запуск",
    "депло",
    "настро",
    "установ",
    "почин",
    "исправ",
    "реализ",
    "обнов",
    "правк",
    "записать файл",
    "как настро",
    "как запуст",
    "как обнов",
];

const FRESHNESS_TASK_CUES: &[&str] = &[
    "when",
    "release",
    "latest",
    "current",
    "today",
    "now",
    "already",
    "available",
    "watch",
    "subscription",
    "price",
    "status",
    "когда",
    "релиз",
    "выйдет",
    "вышел",
    "уже",
    "сейчас",
    "сегодня",
    "последн",
    "текущ",
    "доступ",
    "посмотр",
    "подписк",
    "цена",
    "статус",
];

fn outcome_label(outcome: EpisodeOutcome) -> &'static str {
    match outcome {
        EpisodeOutcome::Success => "success",
        EpisodeOutcome::Failure => "failure",
        EpisodeOutcome::Partial => "partial",
        EpisodeOutcome::Cancelled => "cancelled",
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    }
}
