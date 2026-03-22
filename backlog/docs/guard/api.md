# Guardian API Reference

Complete API documentation for implementing Guardian in your application.

## Core Types

### GuardianApprovalRequest

The main request type that Guardian evaluates.

```rust
// In: guardian/approval_request.rs

pub enum GuardianApprovalRequest {
    /// Shell command execution
    Shell {
        id: String,
        command: Vec<String>,
        cwd: PathBuf,
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<PermissionsOverride>,
        justification: Option<String>,
    },

    /// Exec command with TTY support
    ExecCommand {
        id: String,
        command: Vec<String>,
        cwd: PathBuf,
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<PermissionsOverride>,
        justification: Option<String>,
        tty: Option<bool>,
    },

    /// Unix execve (Unix only)
    #[cfg(unix)]
    Execve {
        tool_name: String,
        program: String,
        argv: Vec<String>,
        cwd: PathBuf,
        additional_permissions: Option<PermissionsOverride>,
    },

    /// Apply patch to files
    ApplyPatch {
        cwd: PathBuf,
        files: Vec<PathBuf>,
        change_count: usize,
        patch: String,
    },

    /// Network access request
    NetworkAccess {
        id: String,
        turn_id: String,
        target: String,
        host: String,
        protocol: String,
        port: u16,
    },

    /// MCP server tool call
    McpToolCall {
        id: String,
        server: String,
        tool_name: String,
        arguments: Option<Value>,
        connector_id: Option<String>,
        connector_name: Option<String>,
        connector_description: Option<String>,
        tool_title: Option<String>,
        tool_description: Option<String>,
        annotations: Option<McpToolAnnotations>,
    },
}
```

### GuardianAssessment

The result of Guardian's evaluation.

```rust
// In: guardian/mod.rs

pub struct GuardianAssessment {
    pub risk_level: GuardianRiskLevel,
    pub risk_score: u8,          // 0-100
    pub rationale: String,
    pub evidence: Vec<GuardianEvidence>,
}

pub struct GuardianEvidence {
    pub message: String,         // Specific fact
    pub why: String,             // Why this matters
}

pub enum GuardianRiskLevel {
    Low,
    Medium,
    High,
}
```

### ReviewDecision

The final decision from the review process.

```rust
// In: protocol/src/protocol.rs

pub enum ReviewDecision {
    /// Action is permitted to proceed
    Approved,

    /// Action is blocked, agent continues
    Denied,

    /// Action is blocked, agent waits for user
    Abort,
}
```

### GuardianReviewOutcome

Outcome from the Guardian session.

```rust
// In: guardian/review.rs

pub(super) enum GuardianReviewOutcome {
    Completed(anyhow::Result<GuardianAssessment>),
    TimedOut,
    Aborted,
}
```

### GuardianReviewSessionOutcome

Outcome from the Guardian review session.

```rust
// In: guardian/review_session.rs

pub(crate) enum GuardianReviewSessionOutcome {
    Completed(anyhow::Result<Option<String>>),  // Option<String> = agent message
    TimedOut,
    Aborted,
}
```

## Session Management Types

### GuardianReviewSessionParams

Parameters for running a review.

```rust
// In: guardian/review_session.rs

pub struct GuardianReviewSessionParams {
    pub parent_session: Arc<Session>,
    pub parent_turn: Arc<TurnContext>,
    pub spawn_config: Config,
    pub prompt_items: Vec<UserInput>,
    pub schema: Value,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffortConfig>,
    pub reasoning_summary: ReasoningSummaryConfig,
    pub personality: Option<Personality>,
    pub external_cancel: Option<CancellationToken>,
}
```

### GuardianReviewSession

The actual Guardian session.

```rust
// In: guardian/review_session.rs

pub struct GuardianReviewSession {
    pub codex: Codex,
    pub cancel_token: CancellationToken,
    pub reuse_key: GuardianReviewSessionReuseKey,
    pub has_prior_review: AtomicBool,
    pub review_lock: Mutex<()>,
    pub last_committed_rollout_items: Mutex<Option<Vec<RolloutItem>>>,
}
```

### GuardianReviewSessionReuseKey

Configuration fingerprint for session reuse.

```rust
// In: guardian/review_session.rs

pub struct GuardianReviewSessionReuseKey {
    pub model: String,
    pub model_provider_id: String,
    pub permissions: PermissionsKey,
    pub developer_instructions: Option<String>,
    pub base_instructions: Option<String>,
    pub user_instructions: Option<String>,
    pub compact_prompt: Option<String>,
    pub cwd: Option<PathBuf>,
    pub mcp_servers: Vec<String>,
    pub features: FeaturesKey,
    pub sandbox_executables: Vec<String>,
}
```

### GuardianReviewSessionManager

Manages Guardian session lifecycle.

```rust
// In: guardian/review_session.rs

pub struct GuardianReviewSessionManager {
    state: Arc<Mutex<GuardianReviewSessionState>>,
}

struct GuardianReviewSessionState {
    trunk: Option<Arc<GuardianReviewSession>>,
    ephemeral_reviews: Vec<Arc<GuardianReviewSession>>,
}
```

## Event Types

### GuardianAssessmentEvent

Event sent to main session during review.

```rust
// In: protocol/src/approvals.rs

pub struct GuardianAssessmentEvent {
    pub id: String,                      // Stable review ID
    pub turn_id: String,                  // Associated turn ID
    pub status: GuardianAssessmentStatus,
    pub risk_score: Option<u8>,           // 0-100 (None while in progress)
    pub risk_level: Option<GuardianRiskLevel>,
    pub rationale: Option<String>,
    pub action: Option<JsonValue>,
}

pub enum GuardianAssessmentStatus {
    InProgress,
    Approved,
    Denied,
    Aborted,
}
```

## Configuration Types

### Guardian Session Config

Configuration for the Guardian sub-agent.

```rust
// In: guardian/review_session.rs

fn build_guardian_review_session_config(
    parent_config: &Config,
    model: &str,
    reasoning_effort: Option<ReasoningEffortConfig>,
) -> Config {
    Config {
        // Use specified model
        model: model.to_string(),
        model_provider_id: parent_config.model_provider_id.clone(),

        // Guardian NEVER asks for approval
        permissions: Permissions {
            approval_policy: Constrained::allow_only(AskForApproval::Never),
            sandbox_policy: Constrained::allow_only(
                SandboxPolicy::new_read_only_policy()
            ),
            ..Default::default()
        },

        // Inherit network config (read-only)
        network_proxy: parent_config.network_proxy.clone(),

        // Disable dangerous features
        features: Features {
            spawn_csv: false,
            collab: false,
            web_search_request: false,
            web_search_cached: false,
            ..Default::default()
        },

        // Use policy as instructions
        developer_instructions: guardian_policy_prompt(),

        // Inherit other settings
        ..parent_config.clone()
    }
}
```

## Prompt Types

### GuardianTranscriptEntry

An entry in the transcript for Guardian.

```rust
// In: guardian/prompt.rs

pub struct GuardianTranscriptEntry {
    pub kind: GuardianTranscriptEntryKind,
    pub text: String,
}

pub enum GuardianTranscriptEntryKind {
    User,
    Assistant,
    Tool(String),  // Tool name
}
```

## Constants

```rust
// In: guardian/mod.rs

/// Preferred model for Guardian reviews
const GUARDIAN_PREFERRED_MODEL: &str = "gpt-5.4";

/// Timeout for Guardian review (90 seconds)
pub const GUARDIAN_REVIEW_TIMEOUT: Duration = Duration::from_secs(90);

/// Name for Guardian agent sessions
pub const GUARDIAN_REVIEWER_NAME: &str = "guardian";

/// Risk score threshold: >= 80 means denied
pub const GUARDIAN_APPROVAL_RISK_THRESHOLD: u8 = 80;

/// Token limits for transcript
const GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS: usize = 10_000;
const GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS: usize = 10_000;
const GUARDIAN_MAX_MESSAGE_ENTRY_TOKENS: usize = 2_000;
const GUARDIAN_MAX_TOOL_ENTRY_TOKENS: usize = 1_000;
const GUARDIAN_MAX_ACTION_STRING_TOKENS: usize = 1_000;

/// Max recent entries to include
const GUARDIAN_RECENT_ENTRY_LIMIT: usize = 40;

/// Truncation marker
const TRUNCATION_TAG: &str = "truncated";
```

## JSON Schema for Output

The schema Guardian must return:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "risk_level": {
      "type": "string",
      "enum": ["low", "medium", "high"]
    },
    "risk_score": {
      "type": "integer",
      "minimum": 0,
      "maximum": 100
    },
    "rationale": {
      "type": "string"
    },
    "evidence": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "message": {
            "type": "string"
          },
          "why": {
            "type": "string"
          }
        },
        "required": ["message", "why"]
      }
    }
  },
  "required": ["risk_level", "risk_score", "rationale", "evidence"]
}
```

## Helper Functions

### routes_approval_to_guardian

Check if approval should route to Guardian.

```rust
// In: guardian/review.rs

pub fn routes_approval_to_guardian(turn: &TurnContext) -> bool {
    turn.approval_policy.value() == AskForApproval::OnRequest
        && turn.config.approvals_reviewer == ApprovalsReviewer::GuardianSubagent
}
```

### build_guardian_prompt_items

Build the prompt for Guardian review.

```rust
// In: guardian/prompt.rs

pub fn build_guardian_prompt_items(
    request: &GuardianApprovalRequest,
    transcript: &[ResponseItem],
    retry_reason: Option<&str>,
    schema: &Value,
) -> anyhow::Result<Vec<UserInput>>
```

### parse_guardian_assessment

Parse Guardian's JSON response.

```rust
// In: guardian/prompt.rs

pub fn parse_guardian_assessment(
    text: Option<&str>,
) -> anyhow::Result<GuardianAssessment>
```

### collect_guardian_transcript_entries

Collect transcript entries with token budgets.

```rust
// In: guardian/prompt.rs

pub fn collect_guardian_transcript_entries(
    items: &[ResponseItem],
    max_message_tokens: usize,
    max_tool_tokens: usize,
    max_entries: usize,
) -> Vec<GuardianTranscriptEntry>
```

### truncate_middle

Truncate text keeping prefix and suffix.

```rust
// In: guardian/prompt.rs

pub fn truncate_middle(
    text: &str,
    max_tokens: usize,
    token_estimator: impl Fn(&str) -> usize,
) -> String
```

## Example Usage

```rust
use guardian::{GuardianApprovalRequest, review_approval_request};

// 1. Create approval request
let request = GuardianApprovalRequest::Shell {
    id: "req-123".into(),
    command: vec!["curl".into(), "-X".into(), "POST".into()],
    cwd: PathBuf::from("/project"),
    sandbox_permissions: SandboxPermissions::default(),
    additional_permissions: None,
    justification: Some("User requested API test".into()),
};

// 2. Run review
let decision = review_approval_request(
    &session,
    &turn_context,
    request,
    Some("Sandbox blocked this command".into()),
).await?;

// 3. Handle decision
match decision {
    ReviewDecision::Approved => {
        // Execute the command
    }
    ReviewDecision::Denied => {
        // Return error to agent
        return Err(ToolError::Rejected("Command denied by Guardian".into()));
    }
    ReviewDecision::Abort => {
        // Wait for user
    }
}
```

## Error Types

```rust
// Guardian-specific errors

GuardianError::ReviewFailed(String)     // Review process failed
GuardianError::ParseFailed(String)       // Failed to parse response
GuardianError::Timeout                   // Review timed out
GuardianError::SessionCreationFailed     // Could not create session
GuardianError::InvalidConfig(String)     // Invalid configuration
```
