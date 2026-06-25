//! Post-run memory curator contracts and canonical candidate writer.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::domain::{
    ActiveMemoryGeneration, FrictionPatternId, FrictionPatternKind, LifeEngramOutboxRow,
    LifeEngramOutboxStatus, LifeFrictionPattern, LifeMemoryItem, LifeSupportProtocol,
    LifeTaskState, MemoryAuthority, MemoryItemId, MemoryItemKind, MemoryItemStatus,
    MemorySensitivity, OutboxId, PrincipalUserId, RunId, SupportProtocolId, SupportStateStatus,
    TaskStateId, TaskStateStatus, TimestampMillis, TurnId,
};
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// Result alias for curator candidate writes.
pub type CuratorResult<T> = Result<T, CuratorError>;

/// Curator pipeline errors.
#[derive(Debug, Error)]
pub enum CuratorError {
    /// Storage operation failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Principal has no active generation.
    #[error("life principal {principal_user_id} has no active memory generation for curator write")]
    MissingActiveGeneration {
        /// Principal id.
        principal_user_id: PrincipalUserId,
    },
}

/// Typed candidate kind produced by the post-run curator.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CuratorCandidateKind {
    /// Durable fact candidate.
    DurableFact,
    /// Project decision candidate.
    ProjectDecision,
    /// Operating profile update candidate.
    OperatingProfileUpdate,
    /// Task state update candidate.
    TaskStateUpdate,
    /// Friction pattern candidate.
    FrictionPattern,
    /// Support protocol candidate.
    SupportProtocol,
    /// Ephemeral observation; do not store durably.
    Ephemeral,
    /// Secret-like candidate requiring deny/redaction.
    SecretCandidate,
    /// Skip candidate.
    Skip,
}

/// Canonical durable memory candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedMemoryCandidate {
    /// Optional caller-provided memory id. Writer allocates one when absent.
    pub memory_id: Option<MemoryItemId>,
    /// Canonical memory category.
    pub kind: MemoryItemKind,
    /// Provenance/authority. Curator-suggested rows are written as candidates.
    pub authority: MemoryAuthority,
    /// Canonical memory text. Must already be redacted when sensitivity is `Redacted`.
    pub text: String,
    /// Structured metadata.
    pub structured: Value,
    /// Tags for later filtering.
    pub tags: Vec<String>,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Sensitivity classification supplied by the structured curator/gate.
    pub sensitivity: MemorySensitivity,
    /// Valid-time start.
    pub valid_from: Option<TimestampMillis>,
    /// Valid-time end.
    pub valid_to: Option<TimestampMillis>,
    /// Superseded memory id.
    pub supersedes_memory_id: Option<MemoryItemId>,
    /// Whether an active clean/redacted row should be projected to Engram outbox.
    pub project_to_engram: bool,
}

/// Candidate patch for deterministic profile/operating state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedProfileUpdateCandidate {
    /// Human-readable candidate summary.
    pub summary: String,
    /// Patch target.
    pub target: ProfileUpdateTarget,
    /// Candidate patch payload. This is not applied by the curator writer.
    pub patch: Value,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
}

/// Profile candidate target.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileUpdateTarget {
    /// `life_principals.profile_state`.
    ProfileState,
    /// `life_principals.operating_profile`.
    OperatingProfile,
}

/// Task resume/open-loop candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedTaskStateCandidate {
    /// Optional caller-provided task id. Writer allocates one when absent.
    pub task_state_id: Option<TaskStateId>,
    /// Stable project key.
    pub project_key: String,
    /// Current goal.
    pub current_goal: String,
    /// Why this goal matters.
    pub why: Option<String>,
    /// Current state payload.
    pub current_state: Value,
    /// Next concrete action.
    pub next_action: Option<String>,
    /// Open loops payload.
    pub open_loops: Value,
    /// Blockers payload.
    pub blockers: Value,
    /// Task state lifecycle.
    pub status: TaskStateStatus,
    /// Last source turn.
    pub last_turn_id: Option<TurnId>,
}

/// Friction pattern candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedFrictionPatternCandidate {
    /// Optional caller-provided pattern id. Writer allocates one when absent.
    pub pattern_id: Option<FrictionPatternId>,
    /// Friction pattern kind.
    pub kind: FrictionPatternKind,
    /// Trigger descriptor.
    pub trigger_descriptor: String,
    /// Preferred response payload.
    pub preferred_response: Value,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Provenance/authority. Curator-suggested rows stay candidates.
    pub authority: MemoryAuthority,
}

/// Support protocol candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedSupportProtocolCandidate {
    /// Optional caller-provided protocol id. Writer allocates one when absent.
    pub protocol_id: Option<SupportProtocolId>,
    /// Protocol name.
    pub name: String,
    /// Trigger descriptor.
    pub trigger_descriptor: String,
    /// Ordered steps payload.
    pub steps: Value,
    /// Prompt priority.
    pub priority: i32,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
    /// Provenance/authority. Curator-suggested rows stay candidates.
    pub authority: MemoryAuthority,
}

/// Ephemeral or skipped candidate note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratedNoteCandidate {
    /// Reason this candidate is not persisted.
    pub reason: String,
}

/// Explicit secret-like candidate. This is denied by the sensitivity gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratedSecretCandidate {
    /// Reason or classifier label.
    pub reason: String,
    /// Redacted preview safe for audit logs/tests.
    pub redacted_preview: Option<String>,
    /// Evidence turns.
    pub evidence_turn_ids: Vec<TurnId>,
}

/// Structured curator output candidate. This is not a source-of-truth write by itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CuratorCandidate {
    /// Durable fact memory.
    DurableFact(CuratedMemoryCandidate),
    /// Project decision memory.
    ProjectDecision(CuratedMemoryCandidate),
    /// Profile/operating profile patch candidate.
    OperatingProfileUpdate(CuratedProfileUpdateCandidate),
    /// Task resume/open-loop update.
    TaskStateUpdate(CuratedTaskStateCandidate),
    /// AuDHD friction pattern.
    FrictionPattern(CuratedFrictionPatternCandidate),
    /// AuDHD support protocol.
    SupportProtocol(CuratedSupportProtocolCandidate),
    /// Ephemeral, do not persist.
    Ephemeral(CuratedNoteCandidate),
    /// Secret-like candidate, deny memory/outbox writes.
    SecretCandidate(CuratedSecretCandidate),
    /// Skip.
    Skip(CuratedNoteCandidate),
}

impl CuratorCandidate {
    /// Returns the stable candidate kind.
    #[must_use]
    pub const fn kind(&self) -> CuratorCandidateKind {
        match self {
            Self::DurableFact(_) => CuratorCandidateKind::DurableFact,
            Self::ProjectDecision(_) => CuratorCandidateKind::ProjectDecision,
            Self::OperatingProfileUpdate(_) => CuratorCandidateKind::OperatingProfileUpdate,
            Self::TaskStateUpdate(_) => CuratorCandidateKind::TaskStateUpdate,
            Self::FrictionPattern(_) => CuratorCandidateKind::FrictionPattern,
            Self::SupportProtocol(_) => CuratorCandidateKind::SupportProtocol,
            Self::Ephemeral(_) => CuratorCandidateKind::Ephemeral,
            Self::SecretCandidate(_) => CuratorCandidateKind::SecretCandidate,
            Self::Skip(_) => CuratorCandidateKind::Skip,
        }
    }
}

/// Curator output for one completed run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuratorOutput {
    /// Completed run id.
    pub run_id: RunId,
    /// Structured candidates.
    pub candidates: Vec<CuratorCandidate>,
}

/// Transcript/package handed to a post-run curator implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostRunCuratorRequest {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Completed run id.
    pub run_id: RunId,
    /// Transcript or compact run artifact. Live LLM integration maps this to structured output.
    pub transcript: Value,
}

/// Single-call post-run curator boundary.
#[async_trait]
pub trait PostRunMemoryCurator: Send + Sync {
    /// Produces structured candidates for a completed run. Implementations must not write storage.
    async fn curate(&self, request: PostRunCuratorRequest) -> CuratorResult<CuratorOutput>;
}

/// Deterministic curator used by tests and bootstrapping before a live LLM client is wired.
#[derive(Debug, Clone, Default)]
pub struct StaticMemoryCurator {
    candidates: Vec<CuratorCandidate>,
}

impl StaticMemoryCurator {
    /// Creates a static curator that returns the supplied candidates for every request.
    #[must_use]
    pub fn new(candidates: Vec<CuratorCandidate>) -> Self {
        Self { candidates }
    }
}

#[async_trait]
impl PostRunMemoryCurator for StaticMemoryCurator {
    async fn curate(&self, request: PostRunCuratorRequest) -> CuratorResult<CuratorOutput> {
        Ok(CuratorOutput {
            run_id: request.run_id,
            candidates: self.candidates.clone(),
        })
    }
}

/// Sensitivity gate decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensitivityDecision {
    /// Candidate may be written using this sensitivity label.
    Allow(MemorySensitivity),
    /// Candidate is denied before memory/outbox writes.
    Deny { reason: String },
}

/// Structured sensitivity gate.
#[derive(Debug, Default, Clone, Copy)]
pub struct SensitivityGate;

impl SensitivityGate {
    /// Classifies a structured candidate without parsing model prose.
    #[must_use]
    pub fn classify(candidate: &CuratorCandidate) -> SensitivityDecision {
        match candidate {
            CuratorCandidate::SecretCandidate(secret) => SensitivityDecision::Deny {
                reason: secret.reason.clone(),
            },
            CuratorCandidate::DurableFact(memory) | CuratorCandidate::ProjectDecision(memory) => {
                if memory.sensitivity == MemorySensitivity::SecretBlocked {
                    SensitivityDecision::Deny {
                        reason: "candidate marked secret_blocked".to_owned(),
                    }
                } else {
                    SensitivityDecision::Allow(memory.sensitivity)
                }
            }
            CuratorCandidate::OperatingProfileUpdate(_)
            | CuratorCandidate::TaskStateUpdate(_)
            | CuratorCandidate::FrictionPattern(_)
            | CuratorCandidate::SupportProtocol(_) => {
                SensitivityDecision::Allow(MemorySensitivity::Clean)
            }
            CuratorCandidate::Ephemeral(_) | CuratorCandidate::Skip(_) => {
                SensitivityDecision::Allow(MemorySensitivity::Clean)
            }
        }
    }
}

/// Candidate write report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CuratorWriteReport {
    /// Written canonical memory ids.
    pub memory_item_ids: Vec<MemoryItemId>,
    /// Written task states.
    pub task_state_ids: Vec<TaskStateId>,
    /// Written friction patterns.
    pub friction_pattern_ids: Vec<FrictionPatternId>,
    /// Written support protocols.
    pub support_protocol_ids: Vec<SupportProtocolId>,
    /// Created outbox rows.
    pub outbox_ids: Vec<OutboxId>,
    /// Profile/operating patch candidates recorded as candidate memory rows.
    pub profile_update_candidate_ids: Vec<MemoryItemId>,
    /// Denied candidate reasons.
    pub denied: Vec<String>,
    /// Skipped/ephemeral reasons.
    pub skipped: Vec<String>,
}

/// Store boundary for canonical curator writes.
#[async_trait]
pub trait CuratorWriteStore: Send + Sync {
    /// Loads the active generation pointer before any candidate write.
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>>;

    /// Writes a canonical memory item.
    async fn upsert_memory_item(&self, item: &LifeMemoryItem) -> LifeStorageResult<()>;

    /// Writes a task state.
    async fn upsert_task_state(&self, task_state: &LifeTaskState) -> LifeStorageResult<()>;

    /// Writes a friction pattern.
    async fn upsert_friction_pattern(&self, pattern: &LifeFrictionPattern)
    -> LifeStorageResult<()>;

    /// Writes a support protocol.
    async fn upsert_support_protocol(
        &self,
        protocol: &LifeSupportProtocol,
    ) -> LifeStorageResult<()>;

    /// Writes an Engram outbox projection.
    async fn insert_engram_outbox(&self, row: &LifeEngramOutboxRow) -> LifeStorageResult<()>;
}

#[async_trait]
impl<T> CuratorWriteStore for T
where
    T: LifeStorageRepository,
{
    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
        LifeStorageRepository::active_generation(self, principal_user_id).await
    }

    async fn upsert_memory_item(&self, item: &LifeMemoryItem) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_memory_item(self, item).await
    }

    async fn upsert_task_state(&self, task_state: &LifeTaskState) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_task_state(self, task_state).await
    }

    async fn upsert_friction_pattern(
        &self,
        pattern: &LifeFrictionPattern,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_friction_pattern(self, pattern).await
    }

    async fn upsert_support_protocol(
        &self,
        protocol: &LifeSupportProtocol,
    ) -> LifeStorageResult<()> {
        LifeStorageRepository::upsert_support_protocol(self, protocol).await
    }

    async fn insert_engram_outbox(&self, row: &LifeEngramOutboxRow) -> LifeStorageResult<()> {
        LifeStorageRepository::insert_engram_outbox(self, row).await
    }
}

/// Clock seam for deterministic candidate writer tests.
pub trait CuratorClock: Send + Sync {
    /// Returns current wall-clock time in milliseconds.
    fn now(&self) -> TimestampMillis;
}

/// Canonical candidate writer. It owns ids, active generation lookup, sensitivity, and outbox projection.
pub struct CanonicalMemoryWriter<S, C> {
    store: S,
    clock: C,
}

impl<S, C> CanonicalMemoryWriter<S, C> {
    /// Creates a writer.
    #[must_use]
    pub const fn new(store: S, clock: C) -> Self {
        Self { store, clock }
    }
}

impl<S, C> CanonicalMemoryWriter<S, C>
where
    S: CuratorWriteStore,
    C: CuratorClock,
{
    /// Applies structured curator output to canonical PG memory tables and derived outbox.
    pub async fn apply_curator_output(
        &self,
        principal_user_id: PrincipalUserId,
        output: CuratorOutput,
    ) -> CuratorResult<CuratorWriteReport> {
        let active_generation = self
            .store
            .active_generation(principal_user_id)
            .await?
            .ok_or(CuratorError::MissingActiveGeneration { principal_user_id })?;

        let mut report = CuratorWriteReport::default();
        for candidate in output.candidates {
            match SensitivityGate::classify(&candidate) {
                SensitivityDecision::Deny { reason } => {
                    report.denied.push(reason);
                    continue;
                }
                SensitivityDecision::Allow(_) => {}
            }

            match candidate {
                CuratorCandidate::DurableFact(memory)
                | CuratorCandidate::ProjectDecision(memory) => {
                    self.write_memory_candidate(
                        principal_user_id,
                        active_generation,
                        output.run_id,
                        memory,
                        &mut report,
                    )
                    .await?;
                }
                CuratorCandidate::OperatingProfileUpdate(update) => {
                    self.write_profile_update_candidate(
                        principal_user_id,
                        active_generation,
                        update,
                        &mut report,
                    )
                    .await?;
                }
                CuratorCandidate::TaskStateUpdate(task) => {
                    self.write_task_state(principal_user_id, active_generation, task, &mut report)
                        .await?;
                }
                CuratorCandidate::FrictionPattern(pattern) => {
                    self.write_friction_pattern(
                        principal_user_id,
                        active_generation,
                        output.run_id,
                        pattern,
                        &mut report,
                    )
                    .await?;
                }
                CuratorCandidate::SupportProtocol(protocol) => {
                    self.write_support_protocol(
                        principal_user_id,
                        active_generation,
                        output.run_id,
                        protocol,
                        &mut report,
                    )
                    .await?;
                }
                CuratorCandidate::Ephemeral(note) | CuratorCandidate::Skip(note) => {
                    report.skipped.push(note.reason);
                }
                CuratorCandidate::SecretCandidate(_) => {}
            }
        }
        Ok(report)
    }

    async fn write_memory_candidate(
        &self,
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        run_id: RunId,
        candidate: CuratedMemoryCandidate,
        report: &mut CuratorWriteReport,
    ) -> CuratorResult<()> {
        let now = self.clock.now();
        let memory_id = candidate.memory_id.unwrap_or_else(MemoryItemId::new_v4);
        let status = memory_status_for_authority(candidate.authority);
        let item = LifeMemoryItem {
            memory_id,
            principal_user_id,
            memory_generation_id: active_generation.scope.memory_generation_id,
            kind: candidate.kind,
            authority: candidate.authority,
            status,
            text: candidate.text,
            structured: candidate.structured,
            tags: candidate.tags,
            evidence_turn_ids: candidate.evidence_turn_ids,
            sensitivity: candidate.sensitivity,
            valid_from: candidate.valid_from,
            valid_to: candidate.valid_to,
            supersedes_memory_id: candidate.supersedes_memory_id,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_memory_item(&item).await?;
        report.memory_item_ids.push(memory_id);

        if item.status == MemoryItemStatus::Active
            && item.sensitivity != MemorySensitivity::SecretBlocked
            && candidate.project_to_engram
        {
            let outbox = outbox_for_memory(&item, run_id, now);
            self.store.insert_engram_outbox(&outbox).await?;
            report.outbox_ids.push(outbox.outbox_id);
        }
        Ok(())
    }

    async fn write_profile_update_candidate(
        &self,
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        update: CuratedProfileUpdateCandidate,
        report: &mut CuratorWriteReport,
    ) -> CuratorResult<()> {
        let now = self.clock.now();
        let memory_id = MemoryItemId::new_v4();
        let item = LifeMemoryItem {
            memory_id,
            principal_user_id,
            memory_generation_id: active_generation.scope.memory_generation_id,
            kind: MemoryItemKind::OperatingRule,
            authority: MemoryAuthority::CuratorSuggested,
            status: MemoryItemStatus::Candidate,
            text: update.summary,
            structured: json!({
                "candidate_type": "profile_update",
                "target": match update.target {
                    ProfileUpdateTarget::ProfileState => "profile_state",
                    ProfileUpdateTarget::OperatingProfile => "operating_profile",
                },
                "patch": update.patch,
            }),
            tags: vec!["profile_update_candidate".to_owned()],
            evidence_turn_ids: update.evidence_turn_ids,
            sensitivity: MemorySensitivity::Clean,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_memory_item(&item).await?;
        report.profile_update_candidate_ids.push(memory_id);
        report.memory_item_ids.push(memory_id);
        Ok(())
    }

    async fn write_task_state(
        &self,
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        candidate: CuratedTaskStateCandidate,
        report: &mut CuratorWriteReport,
    ) -> CuratorResult<()> {
        let now = self.clock.now();
        let task_state_id = candidate.task_state_id.unwrap_or_else(TaskStateId::new_v4);
        let task = LifeTaskState {
            task_state_id,
            principal_user_id,
            memory_generation_id: active_generation.scope.memory_generation_id,
            project_key: candidate.project_key,
            current_goal: candidate.current_goal,
            why: candidate.why,
            current_state: candidate.current_state,
            next_action: candidate.next_action,
            open_loops: candidate.open_loops,
            blockers: candidate.blockers,
            status: candidate.status,
            last_turn_id: candidate.last_turn_id,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_task_state(&task).await?;
        report.task_state_ids.push(task_state_id);
        Ok(())
    }

    async fn write_friction_pattern(
        &self,
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        run_id: RunId,
        candidate: CuratedFrictionPatternCandidate,
        report: &mut CuratorWriteReport,
    ) -> CuratorResult<()> {
        let now = self.clock.now();
        let pattern_id = candidate
            .pattern_id
            .unwrap_or_else(FrictionPatternId::new_v4);
        let status = support_status_for_authority(candidate.authority);
        let pattern = LifeFrictionPattern {
            pattern_id,
            principal_user_id,
            memory_generation_id: active_generation.scope.memory_generation_id,
            kind: candidate.kind,
            trigger_descriptor: candidate.trigger_descriptor.clone(),
            preferred_response: candidate.preferred_response.clone(),
            evidence_turn_ids: candidate.evidence_turn_ids.clone(),
            authority: candidate.authority,
            status,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_friction_pattern(&pattern).await?;
        report.friction_pattern_ids.push(pattern_id);

        let memory = CuratedMemoryCandidate {
            memory_id: None,
            kind: MemoryItemKind::FrictionPattern,
            authority: candidate.authority,
            text: candidate.trigger_descriptor,
            structured: json!({ "preferred_response": candidate.preferred_response }),
            tags: vec!["friction_pattern".to_owned()],
            evidence_turn_ids: candidate.evidence_turn_ids,
            sensitivity: MemorySensitivity::Clean,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            project_to_engram: status == SupportStateStatus::Active,
        };
        self.write_memory_candidate(principal_user_id, active_generation, run_id, memory, report)
            .await
    }

    async fn write_support_protocol(
        &self,
        principal_user_id: PrincipalUserId,
        active_generation: ActiveMemoryGeneration,
        run_id: RunId,
        candidate: CuratedSupportProtocolCandidate,
        report: &mut CuratorWriteReport,
    ) -> CuratorResult<()> {
        let now = self.clock.now();
        let protocol_id = candidate
            .protocol_id
            .unwrap_or_else(SupportProtocolId::new_v4);
        let status = support_status_for_authority(candidate.authority);
        let protocol = LifeSupportProtocol {
            protocol_id,
            principal_user_id,
            memory_generation_id: active_generation.scope.memory_generation_id,
            name: candidate.name.clone(),
            trigger_descriptor: candidate.trigger_descriptor.clone(),
            steps: candidate.steps.clone(),
            priority: candidate.priority,
            evidence_turn_ids: candidate.evidence_turn_ids.clone(),
            authority: candidate.authority,
            status,
            created_at: now,
            updated_at: now,
        };
        self.store.upsert_support_protocol(&protocol).await?;
        report.support_protocol_ids.push(protocol_id);

        let memory = CuratedMemoryCandidate {
            memory_id: None,
            kind: MemoryItemKind::SupportProtocol,
            authority: candidate.authority,
            text: format!("{}: {}", candidate.name, candidate.trigger_descriptor),
            structured: json!({ "steps": candidate.steps, "priority": candidate.priority }),
            tags: vec!["support_protocol".to_owned()],
            evidence_turn_ids: candidate.evidence_turn_ids,
            sensitivity: MemorySensitivity::Clean,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            project_to_engram: status == SupportStateStatus::Active,
        };
        self.write_memory_candidate(principal_user_id, active_generation, run_id, memory, report)
            .await
    }
}

fn memory_status_for_authority(authority: MemoryAuthority) -> MemoryItemStatus {
    match authority {
        MemoryAuthority::UserAsserted | MemoryAuthority::UserConfirmed => MemoryItemStatus::Active,
        MemoryAuthority::CuratorSuggested | MemoryAuthority::SystemDerived => {
            MemoryItemStatus::Candidate
        }
    }
}

fn support_status_for_authority(authority: MemoryAuthority) -> SupportStateStatus {
    match authority {
        MemoryAuthority::UserAsserted | MemoryAuthority::UserConfirmed => {
            SupportStateStatus::Active
        }
        MemoryAuthority::CuratorSuggested | MemoryAuthority::SystemDerived => {
            SupportStateStatus::Candidate
        }
    }
}

fn outbox_for_memory(
    item: &LifeMemoryItem,
    run_id: RunId,
    now: TimestampMillis,
) -> LifeEngramOutboxRow {
    let idempotency_key = format!(
        "life:{}:gen:{}:memory:{}",
        item.principal_user_id, item.memory_generation_id, item.memory_id
    );
    LifeEngramOutboxRow {
        outbox_id: OutboxId::new_v4(),
        principal_user_id: item.principal_user_id,
        memory_generation_id: item.memory_generation_id,
        source_memory_id: Some(item.memory_id),
        idempotency_key,
        payload: json!({
            "external_id": item.memory_id.to_string(),
            "run_id": run_id.to_string(),
            "kind": format!("{:?}", item.kind),
            "text": item.text,
            "structured": item.structured,
            "tags": item.tags,
            "evidence_turn_ids": item.evidence_turn_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "sensitivity": format!("{:?}", item.sensitivity),
        }),
        status: LifeEngramOutboxStatus::Pending,
        attempts: 0,
        next_attempt_at: now,
        last_error: None,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::MemoryScope;

    #[derive(Default)]
    struct FakeCuratorStore {
        active_generation: Mutex<Option<ActiveMemoryGeneration>>,
        memory_items: Mutex<Vec<LifeMemoryItem>>,
        task_states: Mutex<Vec<LifeTaskState>>,
        friction_patterns: Mutex<Vec<LifeFrictionPattern>>,
        support_protocols: Mutex<Vec<LifeSupportProtocol>>,
        outbox: Mutex<Vec<LifeEngramOutboxRow>>,
    }

    #[async_trait]
    impl CuratorWriteStore for FakeCuratorStore {
        async fn active_generation(
            &self,
            _principal_user_id: PrincipalUserId,
        ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
            Ok(*self
                .active_generation
                .lock()
                .expect("active generation lock"))
        }

        async fn upsert_memory_item(&self, item: &LifeMemoryItem) -> LifeStorageResult<()> {
            self.memory_items
                .lock()
                .expect("memory items lock")
                .push(item.clone());
            Ok(())
        }

        async fn upsert_task_state(&self, task_state: &LifeTaskState) -> LifeStorageResult<()> {
            self.task_states
                .lock()
                .expect("task states lock")
                .push(task_state.clone());
            Ok(())
        }

        async fn upsert_friction_pattern(
            &self,
            pattern: &LifeFrictionPattern,
        ) -> LifeStorageResult<()> {
            self.friction_patterns
                .lock()
                .expect("friction patterns lock")
                .push(pattern.clone());
            Ok(())
        }

        async fn upsert_support_protocol(
            &self,
            protocol: &LifeSupportProtocol,
        ) -> LifeStorageResult<()> {
            self.support_protocols
                .lock()
                .expect("support protocols lock")
                .push(protocol.clone());
            Ok(())
        }

        async fn insert_engram_outbox(&self, row: &LifeEngramOutboxRow) -> LifeStorageResult<()> {
            self.outbox.lock().expect("outbox lock").push(row.clone());
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct FixedClock(TimestampMillis);

    impl CuratorClock for FixedClock {
        fn now(&self) -> TimestampMillis {
            self.0
        }
    }

    #[tokio::test]
    async fn static_curator_returns_structured_candidates_without_storage_writes() {
        let run_id = RunId::new_v4();
        let curator =
            StaticMemoryCurator::new(vec![CuratorCandidate::Skip(CuratedNoteCandidate {
                reason: "not durable".to_owned(),
            })]);

        let output = curator
            .curate(PostRunCuratorRequest {
                principal_user_id: PrincipalUserId::new(1).expect("principal"),
                run_id,
                transcript: json!({"turns": []}),
            })
            .await
            .expect("static curator output");

        assert_eq!(output.run_id, run_id);
        assert_eq!(output.candidates.len(), 1);
        assert_eq!(output.candidates[0].kind(), CuratorCandidateKind::Skip);
    }

    #[tokio::test]
    async fn writer_persists_explicit_memory_before_outbox_projection() {
        let principal = PrincipalUserId::new(42).expect("principal");
        let generation = ActiveMemoryGeneration {
            scope: MemoryScope {
                principal_user_id: principal,
                memory_generation_id: crate::domain::MemoryGenerationId::new_v4(),
            },
            activated_at: TimestampMillis::new(10),
        };
        let store = FakeCuratorStore::default();
        *store.active_generation.lock().expect("active lock") = Some(generation);
        let writer = CanonicalMemoryWriter::new(store, FixedClock(TimestampMillis::new(100)));
        let run_id = RunId::new_v4();

        let report = writer
            .apply_curator_output(
                principal,
                CuratorOutput {
                    run_id,
                    candidates: vec![CuratorCandidate::ProjectDecision(CuratedMemoryCandidate {
                        memory_id: None,
                        kind: MemoryItemKind::ProjectPrinciple,
                        authority: MemoryAuthority::UserAsserted,
                        text: "Postgres is source of truth".to_owned(),
                        structured: json!({"project":"oxide-agent"}),
                        tags: vec!["oxide-agent".to_owned()],
                        evidence_turn_ids: vec![TurnId::new_v4()],
                        sensitivity: MemorySensitivity::Clean,
                        valid_from: None,
                        valid_to: None,
                        supersedes_memory_id: None,
                        project_to_engram: true,
                    })],
                },
            )
            .await
            .expect("write report");

        assert_eq!(report.memory_item_ids.len(), 1);
        assert_eq!(report.outbox_ids.len(), 1);
        let memory = writer
            .store
            .memory_items
            .lock()
            .expect("memory lock")
            .first()
            .expect("memory item")
            .clone();
        assert_eq!(memory.status, MemoryItemStatus::Active);
        assert_eq!(
            memory.memory_generation_id,
            generation.scope.memory_generation_id
        );

        let outbox = writer
            .store
            .outbox
            .lock()
            .expect("outbox lock")
            .first()
            .expect("outbox row")
            .clone();
        assert_eq!(outbox.source_memory_id, Some(memory.memory_id));
        assert!(
            outbox
                .idempotency_key
                .contains(&memory.memory_id.to_string())
        );
    }

    #[tokio::test]
    async fn secret_candidate_is_denied_before_memory_or_outbox_writes() {
        let principal = PrincipalUserId::new(7).expect("principal");
        let generation = ActiveMemoryGeneration {
            scope: MemoryScope {
                principal_user_id: principal,
                memory_generation_id: crate::domain::MemoryGenerationId::new_v4(),
            },
            activated_at: TimestampMillis::new(10),
        };
        let store = FakeCuratorStore::default();
        *store.active_generation.lock().expect("active lock") = Some(generation);
        let writer = CanonicalMemoryWriter::new(store, FixedClock(TimestampMillis::new(100)));

        let report = writer
            .apply_curator_output(
                principal,
                CuratorOutput {
                    run_id: RunId::new_v4(),
                    candidates: vec![CuratorCandidate::SecretCandidate(CuratedSecretCandidate {
                        reason: "api_key".to_owned(),
                        redacted_preview: Some("sk-***".to_owned()),
                        evidence_turn_ids: vec![TurnId::new_v4()],
                    })],
                },
            )
            .await
            .expect("write report");

        assert_eq!(report.denied, vec!["api_key".to_owned()]);
        assert!(
            writer
                .store
                .memory_items
                .lock()
                .expect("memory lock")
                .is_empty()
        );
        assert!(writer.store.outbox.lock().expect("outbox lock").is_empty());
    }

    #[tokio::test]
    async fn curator_suggested_profile_update_is_candidate_not_direct_profile_write() {
        let principal = PrincipalUserId::new(8).expect("principal");
        let generation = ActiveMemoryGeneration {
            scope: MemoryScope {
                principal_user_id: principal,
                memory_generation_id: crate::domain::MemoryGenerationId::new_v4(),
            },
            activated_at: TimestampMillis::new(10),
        };
        let store = FakeCuratorStore::default();
        *store.active_generation.lock().expect("active lock") = Some(generation);
        let writer = CanonicalMemoryWriter::new(store, FixedClock(TimestampMillis::new(100)));

        let report = writer
            .apply_curator_output(
                principal,
                CuratorOutput {
                    run_id: RunId::new_v4(),
                    candidates: vec![CuratorCandidate::OperatingProfileUpdate(
                        CuratedProfileUpdateCandidate {
                            summary: "Prefer one next action".to_owned(),
                            target: ProfileUpdateTarget::OperatingProfile,
                            patch: json!({"communication":{"prefer":["one_next_action"]}}),
                            evidence_turn_ids: vec![TurnId::new_v4()],
                        },
                    )],
                },
            )
            .await
            .expect("write report");

        assert_eq!(report.profile_update_candidate_ids.len(), 1);
        assert!(report.outbox_ids.is_empty());
        let memory = writer
            .store
            .memory_items
            .lock()
            .expect("memory lock")
            .first()
            .expect("profile candidate")
            .clone();
        assert_eq!(memory.status, MemoryItemStatus::Candidate);
        assert_eq!(memory.authority, MemoryAuthority::CuratorSuggested);
        assert_eq!(memory.kind, MemoryItemKind::OperatingRule);
    }

    #[tokio::test]
    async fn friction_and_protocol_candidates_write_support_tables_and_candidate_memory() {
        let principal = PrincipalUserId::new(9).expect("principal");
        let generation = ActiveMemoryGeneration {
            scope: MemoryScope {
                principal_user_id: principal,
                memory_generation_id: crate::domain::MemoryGenerationId::new_v4(),
            },
            activated_at: TimestampMillis::new(10),
        };
        let store = FakeCuratorStore::default();
        *store.active_generation.lock().expect("active lock") = Some(generation);
        let writer = CanonicalMemoryWriter::new(store, FixedClock(TimestampMillis::new(100)));

        let report = writer
            .apply_curator_output(
                principal,
                CuratorOutput {
                    run_id: RunId::new_v4(),
                    candidates: vec![
                        CuratorCandidate::FrictionPattern(CuratedFrictionPatternCandidate {
                            pattern_id: None,
                            kind: FrictionPatternKind::OverloadTrigger,
                            trigger_descriptor: "Too many branches".to_owned(),
                            preferred_response: json!(["stop", "one_next_action"]),
                            evidence_turn_ids: vec![TurnId::new_v4()],
                            authority: MemoryAuthority::CuratorSuggested,
                        }),
                        CuratorCandidate::SupportProtocol(CuratedSupportProtocolCandidate {
                            protocol_id: None,
                            name: "Overload narrowing".to_owned(),
                            trigger_descriptor: "User says too many branches".to_owned(),
                            steps: json!(["summarize", "one_next_action"]),
                            priority: 10,
                            evidence_turn_ids: vec![TurnId::new_v4()],
                            authority: MemoryAuthority::UserConfirmed,
                        }),
                    ],
                },
            )
            .await
            .expect("write report");

        assert_eq!(report.friction_pattern_ids.len(), 1);
        assert_eq!(report.support_protocol_ids.len(), 1);
        assert_eq!(report.memory_item_ids.len(), 2);
        assert_eq!(
            report.outbox_ids.len(),
            1,
            "only confirmed protocol is active"
        );
        assert_eq!(
            writer
                .store
                .friction_patterns
                .lock()
                .expect("friction lock")
                .first()
                .expect("friction")
                .status,
            SupportStateStatus::Candidate
        );
        assert_eq!(
            writer
                .store
                .support_protocols
                .lock()
                .expect("support lock")
                .first()
                .expect("support")
                .status,
            SupportStateStatus::Active
        );
    }
}
