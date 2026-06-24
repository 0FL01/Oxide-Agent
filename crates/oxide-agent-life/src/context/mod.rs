//! Life prompt-context assembly.
//!
//! The assembler is intentionally transport- and runtime-neutral. It owns the
//! AuDHD-first memory precedence contract and always resolves the active memory
//! generation itself before reading generation-owned memory rows.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::domain::{
    ActiveMemoryGeneration, LifeContextOverride, LifeFrictionPattern, LifeMemoryItem,
    LifePrincipal, LifeSupportProtocol, LifeTaskState, MemorySensitivity, PrincipalUserId,
    TimestampMillis,
};
use crate::storage::{LifeStorageError, LifeStorageRepository};

/// Result alias for life context assembly.
pub type LifeContextResult<T> = Result<T, LifeContextError>;

/// Context assembly failures.
#[derive(Debug, Error)]
pub enum LifeContextError {
    /// Durable storage failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Principal envelope is missing.
    #[error("life principal {principal_user_id} is missing")]
    MissingPrincipal {
        /// Principal requested by the worker/runtime.
        principal_user_id: PrincipalUserId,
    },
    /// Principal has no active memory generation.
    #[error("life principal {principal_user_id} has no active memory generation")]
    MissingActiveGeneration {
        /// Principal requested by the worker/runtime.
        principal_user_id: PrincipalUserId,
    },
    /// JSON rendering failed while preparing prompt context.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Request to assemble life prompt context for a principal/run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeContextRequest {
    /// Principal owner. The assembler resolves the active generation itself.
    pub principal_user_id: PrincipalUserId,
    /// Current wall-clock timestamp for TTL override filtering.
    pub now: TimestampMillis,
    /// Optional project key to bias task resume rendering.
    pub project_key: Option<String>,
    /// User query/task text used by future recall/relevance filtering.
    pub query: String,
    /// Optional hot checkpoint/handoff text supplied by the worker.
    pub hot_handoff: Option<String>,
}

/// PRD-defined prompt block names emitted by the life context provider.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeContextBlockKind {
    /// Life defaults/profile block.
    LifeDefaults,
    /// Confirmed AuDHD operating contract.
    OperatingContract,
    /// Current task resume/open-loop block.
    CurrentTaskResume,
    /// Temporary active overrides.
    ActiveOverrides,
    /// Support protocols and friction patterns.
    SupportProtocols,
    /// Hot checkpoint handoff.
    HotHandoff,
    /// Long-term memory evidence.
    LongTermMemoryEvidence,
}

impl LifeContextBlockKind {
    /// Stable block name for diagnostics and eventual core prompt conversion.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LifeDefaults => "life_defaults",
            Self::OperatingContract => "audhd_operating_contract",
            Self::CurrentTaskResume => "current_task_resume",
            Self::ActiveOverrides => "active_temporary_overrides",
            Self::SupportProtocols => "support_protocols",
            Self::HotHandoff => "hot_handoff",
            Self::LongTermMemoryEvidence => "long_term_memory_evidence",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::LifeDefaults => "## Life Defaults",
            Self::OperatingContract => "## AuDHD Operating Contract",
            Self::CurrentTaskResume => "## Current Task Resume",
            Self::ActiveOverrides => "## Active Temporary Overrides",
            Self::SupportProtocols => "## Support Protocols and Friction Patterns",
            Self::HotHandoff => "## Hot Handoff",
            Self::LongTermMemoryEvidence => "## Long-Term Memory Evidence",
        }
    }

    fn semantics(self) -> LifeContextBlockSemantics {
        match self {
            Self::LifeDefaults => LifeContextBlockSemantics::AuthoritativeUserDefault,
            Self::OperatingContract => LifeContextBlockSemantics::OperatingContract,
            Self::CurrentTaskResume => LifeContextBlockSemantics::TaskResume,
            Self::ActiveOverrides => LifeContextBlockSemantics::DeterministicRuleLike,
            Self::SupportProtocols => LifeContextBlockSemantics::SupportProtocol,
            Self::HotHandoff | Self::LongTermMemoryEvidence => {
                LifeContextBlockSemantics::EvidenceOnly
            }
        }
    }
}

/// Meaning of a life context block. This mirrors the core prompt semantics
/// without forcing this bounded context to depend on the execution crate.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeContextBlockSemantics {
    /// Deterministic rule-like context that constrains runtime behavior.
    DeterministicRuleLike,
    /// Authoritative user default or preference confirmed outside recall.
    AuthoritativeUserDefault,
    /// Confirmed operating contract for how to work with this user.
    OperatingContract,
    /// Current task resume/open-loop state.
    TaskResume,
    /// Reusable support protocol selected for the current turn.
    SupportProtocol,
    /// Evidence/background context, not an instruction source.
    EvidenceOnly,
}

/// One assembled life prompt context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifePromptContextBlock {
    /// PRD-defined block kind.
    pub kind: LifeContextBlockKind,
    /// Stable block name.
    pub name: String,
    /// Markdown body to insert into prompt context.
    pub body: String,
    /// Prompt precedence semantics.
    pub semantics: LifeContextBlockSemantics,
}

impl LifePromptContextBlock {
    fn new(kind: LifeContextBlockKind, body: String) -> Self {
        Self {
            kind,
            name: kind.name().to_owned(),
            body,
            semantics: kind.semantics(),
        }
    }
}

/// Fully assembled life context pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeContextPack {
    /// Active generation used for every generation-owned read.
    pub active_generation: ActiveMemoryGeneration,
    /// Ordered prompt context blocks.
    pub blocks: Vec<LifePromptContextBlock>,
}

/// Narrow storage boundary needed by context assembly.
#[async_trait]
pub trait LifeContextStore: Send + Sync {
    /// Load principal profile and operating envelope.
    async fn principal(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeContextResult<Option<LifePrincipal>>;

    /// Load active memory generation pointer.
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeContextResult<Option<ActiveMemoryGeneration>>;

    /// Load non-expired overrides.
    async fn active_context_overrides(
        &self,
        principal_user_id: PrincipalUserId,
        now: TimestampMillis,
    ) -> LifeContextResult<Vec<LifeContextOverride>>;

    /// Load active task resume packets from the provided active scope.
    async fn active_task_states(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeTaskState>>;

    /// Load active friction patterns from the provided active scope.
    async fn active_friction_patterns(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeFrictionPattern>>;

    /// Load active support protocols from the provided active scope.
    async fn active_support_protocols(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeSupportProtocol>>;

    /// Load active canonical long-term memory from the provided active scope.
    async fn active_memory_items(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeMemoryItem>>;
}

#[async_trait]
impl<T> LifeContextStore for T
where
    T: LifeStorageRepository,
{
    async fn principal(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeContextResult<Option<LifePrincipal>> {
        Ok(LifeStorageRepository::principal(self, principal_user_id).await?)
    }

    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeContextResult<Option<ActiveMemoryGeneration>> {
        Ok(LifeStorageRepository::active_generation(self, principal_user_id).await?)
    }

    async fn active_context_overrides(
        &self,
        principal_user_id: PrincipalUserId,
        now: TimestampMillis,
    ) -> LifeContextResult<Vec<LifeContextOverride>> {
        Ok(LifeStorageRepository::active_context_overrides(self, principal_user_id, now).await?)
    }

    async fn active_task_states(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeTaskState>> {
        Ok(LifeStorageRepository::active_task_states(self, active_generation.scope).await?)
    }

    async fn active_friction_patterns(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeFrictionPattern>> {
        Ok(LifeStorageRepository::active_friction_patterns(self, active_generation.scope).await?)
    }

    async fn active_support_protocols(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeSupportProtocol>> {
        Ok(LifeStorageRepository::active_support_protocols(self, active_generation.scope).await?)
    }

    async fn active_memory_items(
        &self,
        active_generation: ActiveMemoryGeneration,
    ) -> LifeContextResult<Vec<LifeMemoryItem>> {
        Ok(LifeStorageRepository::active_memory_items(self, active_generation.scope).await?)
    }
}

/// Assembles AuDHD-first life prompt context in PRD precedence order.
pub struct LifeContextAssembler<S> {
    store: S,
}

impl<S> LifeContextAssembler<S> {
    /// Creates an assembler over the provided store.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// Returns the underlying store.
    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }
}

impl<S> LifeContextAssembler<S>
where
    S: LifeContextStore,
{
    /// Builds ordered context blocks for one life run.
    pub async fn assemble(
        &self,
        request: LifeContextRequest,
    ) -> LifeContextResult<LifeContextPack> {
        let principal = self
            .store
            .principal(request.principal_user_id)
            .await?
            .ok_or(LifeContextError::MissingPrincipal {
                principal_user_id: request.principal_user_id,
            })?;
        let active_generation = self
            .store
            .active_generation(request.principal_user_id)
            .await?
            .ok_or(LifeContextError::MissingActiveGeneration {
                principal_user_id: request.principal_user_id,
            })?;

        let task_states = self.store.active_task_states(active_generation).await?;
        let overrides = self
            .store
            .active_context_overrides(request.principal_user_id, request.now)
            .await?;
        let friction_patterns = self
            .store
            .active_friction_patterns(active_generation)
            .await?;
        let support_protocols = self
            .store
            .active_support_protocols(active_generation)
            .await?;
        let memory_items = self.store.active_memory_items(active_generation).await?;

        let mut blocks = Vec::new();
        push_json_block(
            &mut blocks,
            LifeContextBlockKind::LifeDefaults,
            &principal.profile_state,
        )?;
        push_json_block(
            &mut blocks,
            LifeContextBlockKind::OperatingContract,
            &principal.operating_profile,
        )?;
        push_string_block(
            &mut blocks,
            LifeContextBlockKind::CurrentTaskResume,
            render_task_states(task_states, request.project_key.as_deref())?,
        );
        push_string_block(
            &mut blocks,
            LifeContextBlockKind::ActiveOverrides,
            render_overrides(overrides)?,
        );
        push_string_block(
            &mut blocks,
            LifeContextBlockKind::SupportProtocols,
            render_support_context(support_protocols, friction_patterns)?,
        );
        push_string_block(
            &mut blocks,
            LifeContextBlockKind::HotHandoff,
            request.hot_handoff.unwrap_or_default(),
        );
        push_string_block(
            &mut blocks,
            LifeContextBlockKind::LongTermMemoryEvidence,
            render_memory_evidence(memory_items),
        );

        Ok(LifeContextPack {
            active_generation,
            blocks,
        })
    }
}

fn push_json_block(
    blocks: &mut Vec<LifePromptContextBlock>,
    kind: LifeContextBlockKind,
    value: &Value,
) -> LifeContextResult<()> {
    if is_effectively_empty_json(value) {
        return Ok(());
    }
    let body = format!(
        "{}\n{}",
        kind.heading(),
        serde_json::to_string_pretty(value)?
    );
    blocks.push(LifePromptContextBlock::new(kind, body));
    Ok(())
}

fn push_string_block(
    blocks: &mut Vec<LifePromptContextBlock>,
    kind: LifeContextBlockKind,
    body: String,
) {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return;
    }
    blocks.push(LifePromptContextBlock::new(
        kind,
        format!("{}\n{}", kind.heading(), trimmed),
    ));
}

fn is_effectively_empty_json(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn render_task_states(
    mut task_states: Vec<LifeTaskState>,
    project_key: Option<&str>,
) -> LifeContextResult<String> {
    task_states.sort_by(|left, right| {
        let left_match = project_key.is_some_and(|key| left.project_key == key);
        let right_match = project_key.is_some_and(|key| right.project_key == key);
        right_match
            .cmp(&left_match)
            .then_with(|| left.project_key.cmp(&right.project_key))
    });

    let mut lines = Vec::new();
    for task in task_states {
        lines.push(format!("- Project: {}", task.project_key));
        lines.push(format!("  - Current goal: {}", task.current_goal));
        if let Some(why) = non_empty(task.why.as_deref()) {
            lines.push(format!("  - Why: {why}"));
        }
        if let Some(next_action) = non_empty(task.next_action.as_deref()) {
            lines.push(format!("  - Next action: {next_action}"));
        }
        push_json_line(&mut lines, "Current state", &task.current_state)?;
        push_json_line(&mut lines, "Open loops", &task.open_loops)?;
        push_json_line(&mut lines, "Blockers", &task.blockers)?;
    }
    Ok(lines.join("\n"))
}

fn render_overrides(overrides: Vec<LifeContextOverride>) -> LifeContextResult<String> {
    let mut lines = Vec::new();
    for context_override in overrides {
        lines.push(format!(
            "- {}: {}",
            context_override.key,
            serde_json::to_string(&context_override.value)?
        ));
        if let Some(reason) = non_empty(context_override.reason.as_deref()) {
            lines.push(format!("  - Reason: {reason}"));
        }
        if let Some(expires_at) = context_override.expires_at {
            lines.push(format!("  - Expires at: {}", expires_at.get()));
        }
    }
    Ok(lines.join("\n"))
}

fn render_support_context(
    mut protocols: Vec<LifeSupportProtocol>,
    mut patterns: Vec<LifeFrictionPattern>,
) -> LifeContextResult<String> {
    protocols.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.name.cmp(&right.name))
    });
    patterns.sort_by(|left, right| left.trigger_descriptor.cmp(&right.trigger_descriptor));

    let mut lines = Vec::new();
    for protocol in protocols {
        lines.push(format!(
            "- Protocol: {} (trigger: {})",
            protocol.name, protocol.trigger_descriptor
        ));
        push_json_line(&mut lines, "Steps", &protocol.steps)?;
    }
    for pattern in patterns {
        lines.push(format!(
            "- Friction pattern: {} ({:?})",
            pattern.trigger_descriptor, pattern.kind
        ));
        push_json_line(
            &mut lines,
            "Preferred response",
            &pattern.preferred_response,
        )?;
    }
    Ok(lines.join("\n"))
}

fn render_memory_evidence(memory_items: Vec<LifeMemoryItem>) -> String {
    let mut lines = Vec::new();
    for item in memory_items
        .into_iter()
        .filter(|item| item.sensitivity != MemorySensitivity::SecretBlocked)
    {
        lines.push(format!(
            "- [{:?}, {:?}] {}",
            item.kind, item.authority, item.text
        ));
    }
    lines.join("\n")
}

fn push_json_line(lines: &mut Vec<String>, label: &str, value: &Value) -> LifeContextResult<()> {
    if is_effectively_empty_json(value) {
        return Ok(());
    }
    lines.push(format!("  - {label}: {}", serde_json::to_string(value)?));
    Ok(())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;

    use crate::domain::{
        ActiveMemoryGeneration, FrictionPatternId, FrictionPatternKind, LifeFrictionPattern,
        LifeMemoryItem, LifePrincipal, LifeSupportProtocol, LifeTaskState, MemoryAuthority,
        MemoryGenerationId, MemoryItemId, MemoryItemKind, MemoryItemStatus, MemoryScope,
        MemorySensitivity, PrincipalUserId, SupportProtocolId, SupportStateStatus, TaskStateId,
        TaskStateStatus, TimestampMillis,
    };

    use super::*;

    #[tokio::test]
    async fn assembler_preserves_prd_block_order_and_semantics() {
        let principal_user_id = principal();
        let generation_id = MemoryGenerationId::new_v4();
        let store = FakeContextStore::new(principal_user_id, generation_id)
            .with_principal(default_principal(principal_user_id))
            .with_task(task_state(
                principal_user_id,
                generation_id,
                "life",
                "Update PRD",
            ))
            .with_override(LifeContextOverride {
                override_id: crate::domain::ContextOverrideId::new_v4(),
                principal_user_id,
                key: "answer_verbosity".to_owned(),
                value: json!("detailed"),
                reason: Some("today only".to_owned()),
                expires_at: None,
                created_at: ts(1),
                updated_at: ts(2),
            })
            .with_protocol(protocol(
                principal_user_id,
                generation_id,
                "Context restore",
                10,
            ))
            .with_pattern(pattern(principal_user_id, generation_id, "option overload"))
            .with_memory(memory_item(
                principal_user_id,
                generation_id,
                "Postgres is source of truth",
                MemorySensitivity::Clean,
            ));

        let pack = LifeContextAssembler::new(store)
            .assemble(request(
                principal_user_id,
                Some("life"),
                Some("handoff summary"),
            ))
            .await
            .expect("assemble context");

        let kinds: Vec<_> = pack.blocks.iter().map(|block| block.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LifeContextBlockKind::LifeDefaults,
                LifeContextBlockKind::OperatingContract,
                LifeContextBlockKind::CurrentTaskResume,
                LifeContextBlockKind::ActiveOverrides,
                LifeContextBlockKind::SupportProtocols,
                LifeContextBlockKind::HotHandoff,
                LifeContextBlockKind::LongTermMemoryEvidence,
            ]
        );
        assert_eq!(
            pack.blocks[1].semantics,
            LifeContextBlockSemantics::OperatingContract
        );
        assert_eq!(
            pack.blocks[2].semantics,
            LifeContextBlockSemantics::TaskResume
        );
        assert_eq!(
            pack.blocks[6].semantics,
            LifeContextBlockSemantics::EvidenceOnly
        );
    }

    #[tokio::test]
    async fn assembler_loads_active_generation_before_scoped_memory_reads() {
        let principal_user_id = principal();
        let active_generation_id = MemoryGenerationId::new_v4();
        let store = FakeContextStore::new(principal_user_id, active_generation_id)
            .with_principal(default_principal(principal_user_id))
            .with_memory(memory_item(
                principal_user_id,
                active_generation_id,
                "active memory only",
                MemorySensitivity::Clean,
            ));
        let calls = store.calls.clone();

        let pack = LifeContextAssembler::new(store)
            .assemble(request(principal_user_id, None, None))
            .await
            .expect("assemble context");

        assert_eq!(
            pack.active_generation.scope.memory_generation_id,
            active_generation_id
        );
        assert!(
            pack.blocks
                .iter()
                .any(|block| block.body.contains("active memory only"))
        );
        assert_eq!(
            calls.lock().expect("calls lock").as_slice(),
            &[
                "principal",
                "active_generation",
                "active_task_states",
                "active_context_overrides",
                "active_friction_patterns",
                "active_support_protocols",
                "active_memory_items",
            ]
        );
    }

    #[tokio::test]
    async fn context_restore_uses_task_resume_without_broad_question_inference() {
        let principal_user_id = principal();
        let generation_id = MemoryGenerationId::new_v4();
        let store = FakeContextStore::new(principal_user_id, generation_id)
            .with_principal(default_principal(principal_user_id))
            .with_task(task_state(
                principal_user_id,
                generation_id,
                "permanent-life-mode",
                "Explain memory read/write flow",
            ));

        let pack = LifeContextAssembler::new(store)
            .assemble(LifeContextRequest {
                principal_user_id,
                now: ts(10),
                project_key: Some("permanent-life-mode".to_owned()),
                query: "Я потерял контекст. С чего начать?".to_owned(),
                hot_handoff: None,
            })
            .await
            .expect("assemble context");
        let resume = block(&pack, LifeContextBlockKind::CurrentTaskResume);
        assert!(resume.body.contains("Explain memory read/write flow"));
        assert!(resume.body.contains("Next action"));
    }

    #[tokio::test]
    async fn overload_protocol_is_rendered_only_from_confirmed_support_state() {
        let principal_user_id = principal();
        let generation_id = MemoryGenerationId::new_v4();
        let store = FakeContextStore::new(principal_user_id, generation_id)
            .with_principal(default_principal(principal_user_id))
            .with_protocol(protocol(
                principal_user_id,
                generation_id,
                "Overload narrowing",
                100,
            ));

        let pack = LifeContextAssembler::new(store)
            .assemble(LifeContextRequest {
                principal_user_id,
                now: ts(10),
                project_key: None,
                query: "слишком много веток, я завис".to_owned(),
                hot_handoff: None,
            })
            .await
            .expect("assemble context");
        let support = block(&pack, LifeContextBlockKind::SupportProtocols);
        assert!(support.body.contains("Overload narrowing"));
        assert_eq!(
            support.semantics,
            LifeContextBlockSemantics::SupportProtocol
        );
    }

    #[tokio::test]
    async fn secret_blocked_memory_is_not_rendered_as_evidence() {
        let principal_user_id = principal();
        let generation_id = MemoryGenerationId::new_v4();
        let store = FakeContextStore::new(principal_user_id, generation_id)
            .with_principal(default_principal(principal_user_id))
            .with_memory(memory_item(
                principal_user_id,
                generation_id,
                "safe project fact",
                MemorySensitivity::Clean,
            ))
            .with_memory(memory_item(
                principal_user_id,
                generation_id,
                "API key sk-secret",
                MemorySensitivity::SecretBlocked,
            ));

        let pack = LifeContextAssembler::new(store)
            .assemble(request(principal_user_id, None, None))
            .await
            .expect("assemble context");
        let evidence = block(&pack, LifeContextBlockKind::LongTermMemoryEvidence);
        assert!(evidence.body.contains("safe project fact"));
        assert!(!evidence.body.contains("sk-secret"));
    }

    fn block(pack: &LifeContextPack, kind: LifeContextBlockKind) -> &LifePromptContextBlock {
        pack.blocks
            .iter()
            .find(|block| block.kind == kind)
            .expect("expected block kind")
    }

    fn request(
        principal_user_id: PrincipalUserId,
        project_key: Option<&str>,
        hot_handoff: Option<&str>,
    ) -> LifeContextRequest {
        LifeContextRequest {
            principal_user_id,
            now: ts(10),
            project_key: project_key.map(str::to_owned),
            query: "test query".to_owned(),
            hot_handoff: hot_handoff.map(str::to_owned),
        }
    }

    fn principal() -> PrincipalUserId {
        PrincipalUserId::new(100500).expect("positive principal")
    }

    fn ts(value: i64) -> TimestampMillis {
        TimestampMillis::new(value)
    }

    fn default_principal(principal_user_id: PrincipalUserId) -> LifePrincipal {
        LifePrincipal {
            principal_user_id,
            profile_state: json!({"communication": {"default_language": "ru"}}),
            operating_profile: json!({
                "task_support": {"role": "external executive-function support"},
                "communication": {"avoid": ["broad_open_questions"]}
            }),
            settings: json!({}),
            schema_version: 1,
            created_at: ts(1),
            updated_at: ts(2),
        }
    }

    fn task_state(
        principal_user_id: PrincipalUserId,
        generation_id: MemoryGenerationId,
        project_key: &str,
        current_goal: &str,
    ) -> LifeTaskState {
        LifeTaskState {
            task_state_id: TaskStateId::new_v4(),
            principal_user_id,
            memory_generation_id: generation_id,
            project_key: project_key.to_owned(),
            current_goal: current_goal.to_owned(),
            why: Some("preserve task continuity".to_owned()),
            current_state: json!(["context assembler phase"]),
            next_action: Some("assemble deterministic context pack".to_owned()),
            open_loops: json!(["active generation filtering"]),
            blockers: json!([]),
            status: TaskStateStatus::Active,
            last_turn_id: None,
            created_at: ts(1),
            updated_at: ts(2),
        }
    }

    fn protocol(
        principal_user_id: PrincipalUserId,
        generation_id: MemoryGenerationId,
        name: &str,
        priority: i32,
    ) -> LifeSupportProtocol {
        LifeSupportProtocol {
            protocol_id: SupportProtocolId::new_v4(),
            principal_user_id,
            memory_generation_id: generation_id,
            name: name.to_owned(),
            trigger_descriptor: "context loss or overload".to_owned(),
            steps: json!(["summarize current state", "give one next action"]),
            priority,
            evidence_turn_ids: Vec::new(),
            authority: MemoryAuthority::UserConfirmed,
            status: SupportStateStatus::Active,
            created_at: ts(1),
            updated_at: ts(2),
        }
    }

    fn pattern(
        principal_user_id: PrincipalUserId,
        generation_id: MemoryGenerationId,
        trigger_descriptor: &str,
    ) -> LifeFrictionPattern {
        LifeFrictionPattern {
            pattern_id: FrictionPatternId::new_v4(),
            principal_user_id,
            memory_generation_id: generation_id,
            kind: FrictionPatternKind::OverloadTrigger,
            trigger_descriptor: trigger_descriptor.to_owned(),
            preferred_response: json!({"next_action_count": 1}),
            evidence_turn_ids: Vec::new(),
            authority: MemoryAuthority::UserConfirmed,
            status: SupportStateStatus::Active,
            created_at: ts(1),
            updated_at: ts(2),
        }
    }

    fn memory_item(
        principal_user_id: PrincipalUserId,
        generation_id: MemoryGenerationId,
        text: &str,
        sensitivity: MemorySensitivity,
    ) -> LifeMemoryItem {
        LifeMemoryItem {
            memory_id: MemoryItemId::new_v4(),
            principal_user_id,
            memory_generation_id: generation_id,
            kind: MemoryItemKind::ProjectPrinciple,
            authority: MemoryAuthority::UserAsserted,
            status: MemoryItemStatus::Active,
            text: text.to_owned(),
            structured: json!({}),
            tags: vec!["oxide-agent".to_owned()],
            evidence_turn_ids: Vec::new(),
            sensitivity,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            created_at: ts(1),
            updated_at: ts(2),
        }
    }

    #[derive(Clone)]
    struct FakeContextStore {
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        principal: Option<LifePrincipal>,
        tasks: Vec<LifeTaskState>,
        overrides: Vec<LifeContextOverride>,
        patterns: Vec<LifeFrictionPattern>,
        protocols: Vec<LifeSupportProtocol>,
        memory_items: Vec<LifeMemoryItem>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl FakeContextStore {
        fn new(principal_user_id: PrincipalUserId, generation_id: MemoryGenerationId) -> Self {
            Self {
                principal_user_id,
                active_generation: ActiveMemoryGeneration {
                    scope: MemoryScope::new(principal_user_id, generation_id),
                    activated_at: ts(1),
                },
                principal: None,
                tasks: Vec::new(),
                overrides: Vec::new(),
                patterns: Vec::new(),
                protocols: Vec::new(),
                memory_items: Vec::new(),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_principal(mut self, principal: LifePrincipal) -> Self {
            self.principal = Some(principal);
            self
        }

        fn with_task(mut self, task: LifeTaskState) -> Self {
            self.tasks.push(task);
            self
        }

        fn with_override(mut self, context_override: LifeContextOverride) -> Self {
            self.overrides.push(context_override);
            self
        }

        fn with_pattern(mut self, pattern: LifeFrictionPattern) -> Self {
            self.patterns.push(pattern);
            self
        }

        fn with_protocol(mut self, protocol: LifeSupportProtocol) -> Self {
            self.protocols.push(protocol);
            self
        }

        fn with_memory(mut self, item: LifeMemoryItem) -> Self {
            self.memory_items.push(item);
            self
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().expect("calls lock").push(call);
        }

        fn assert_active_generation(&self, active_generation: ActiveMemoryGeneration) {
            assert_eq!(active_generation, self.active_generation);
        }
    }

    #[async_trait]
    impl LifeContextStore for FakeContextStore {
        async fn principal(
            &self,
            principal_user_id: PrincipalUserId,
        ) -> LifeContextResult<Option<LifePrincipal>> {
            self.record("principal");
            assert_eq!(principal_user_id, self.principal_user_id);
            Ok(self.principal.clone())
        }

        async fn active_generation(
            &self,
            principal_user_id: PrincipalUserId,
        ) -> LifeContextResult<Option<ActiveMemoryGeneration>> {
            self.record("active_generation");
            assert_eq!(principal_user_id, self.principal_user_id);
            Ok(Some(self.active_generation))
        }

        async fn active_context_overrides(
            &self,
            principal_user_id: PrincipalUserId,
            _now: TimestampMillis,
        ) -> LifeContextResult<Vec<LifeContextOverride>> {
            self.record("active_context_overrides");
            assert_eq!(principal_user_id, self.principal_user_id);
            Ok(self.overrides.clone())
        }

        async fn active_task_states(
            &self,
            active_generation: ActiveMemoryGeneration,
        ) -> LifeContextResult<Vec<LifeTaskState>> {
            self.record("active_task_states");
            self.assert_active_generation(active_generation);
            Ok(self.tasks.clone())
        }

        async fn active_friction_patterns(
            &self,
            active_generation: ActiveMemoryGeneration,
        ) -> LifeContextResult<Vec<LifeFrictionPattern>> {
            self.record("active_friction_patterns");
            self.assert_active_generation(active_generation);
            Ok(self.patterns.clone())
        }

        async fn active_support_protocols(
            &self,
            active_generation: ActiveMemoryGeneration,
        ) -> LifeContextResult<Vec<LifeSupportProtocol>> {
            self.record("active_support_protocols");
            self.assert_active_generation(active_generation);
            Ok(self.protocols.clone())
        }

        async fn active_memory_items(
            &self,
            active_generation: ActiveMemoryGeneration,
        ) -> LifeContextResult<Vec<LifeMemoryItem>> {
            self.record("active_memory_items");
            self.assert_active_generation(active_generation);
            Ok(self.memory_items.clone())
        }
    }
}
