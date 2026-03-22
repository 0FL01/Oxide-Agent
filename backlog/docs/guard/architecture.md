# Guardian Architecture

## System Overview

Guardian состоит из нескольких ключевых компонентов, работающих вместе для обеспечения безопасности.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            MAIN SESSION                                     │
│                                                                             │
│  ┌─────────────────┐     ┌──────────────────────────────────────────────┐  │
│  │    Session      │────▶│           Tool Orchestrator                   │  │
│  │  (parent agent) │     │                                              │  │
│  └─────────────────┘     │  routes_approval_to_guardian(turn_context)?   │  │
│          │               │            ↓                                  │  │
│          │               │  ┌─────────────────────────────────────────┐  │  │
│          │               │  │        Guardian Review                 │  │  │
│          │               │  │  ┌─────────────────────────────────┐  │  │  │
│          │               │  │  │   GuardianReviewSessionManager  │  │  │  │
│          │               │  │  └─────────────────────────────────┘  │  │  │
│          │               │  └─────────────────────────────────────────┘  │  │
│          │               └──────────────────────────────────────────────┘  │
│          │                              │                                   │
│          │               ┌──────────────┴──────────────┐                   │
│          │               │        Events Channel        │                   │
│          │               └──────────────┬──────────────┘                   │
└──────────┼──────────────────────────────┼───────────────────────────────┘
           │                              │
           │         ┌────────────────────┴────────────────────┐
           │         │                                         │
           ▼         ▼                                         ▼
┌─────────────────────┐           ┌─────────────────────────────────────────┐
│ Guardian Assessment  │           │           GUARDIAN SESSION              │
│     Events          │           │                                         │
│                     │           │  ┌─────────────────────────────────────┐ │
│ • InProgress        │           │  │       GuardianReviewSession        │ │
│ • Approved          │           │  │                                     │ │
│ • Denied            │           │  │  • codex: Codex (isolated agent)  │ │
│ • Aborted           │           │  │  • review_lock: Mutex              │ │
│                     │           │  │  • reuse_key: SessionReuseKey      │ │
└─────────────────────┘           │  │  • has_prior_review: AtomicBool   │ │
                                   │  └─────────────────────────────────────┘ │
                                   │                                         │
                                   │  ┌─────────────────────────────────────┐ │
                                   │  │      Session Management             │ │
                                   │  │                                     │ │
                                   │  │  trunk: Option<GuardianSession>    │ │
                                   │  │      ↓                              │ │
                                   │  │  ephemeral_reviews: Vec<Session>   │ │
                                   │  │      (parallel reviews)            │ │
                                   │  └─────────────────────────────────────┘ │
                                   └─────────────────────────────────────────┘
```

## Component Responsibilities

### 1. Session (Main Agent)

Главная сессия агента, которая содержит:

- `guardian_review_session: GuardianReviewSessionManager` — менеджер Guardian сессий
- Все инструменты вызывают `orchestrator.run()`, который проверяет необходимость Guardian review

### 2. ToolOrchestrator

Центральный оркестратор выполнения tool calls:

```rust
pub struct ToolOrchestrator {
    pub tool: Arc<dyn ToolRuntime>,
    pub sandbox_manager: Arc<SandboxManager>,
    pub approval_policy: Constrained<AskForApproval>,
    pub config: Arc<Config>,
}

impl ToolOrchestrator {
    pub async fn run(&self, req: ToolRequest) -> Result<OrchestratorRunResult<Out>, ToolError> {
        // 1. Check if guardian routing needed
        if routes_approval_to_guardian(&turn_context) {
            // 2. Run guardian review
            let decision = review_approval_request(session, turn, request, retry_reason)?;
            match decision {
                ReviewDecision::Approved => { /* continue */ }
                ReviewDecision::Denied => return ToolError::Rejected(...),
                ReviewDecision::Abort => { /* wait for user */ }
            }
        }

        // 3. Execute tool
        self.run_attempt(req, attempt, ctx).await
    }
}
```

### 3. GuardianReviewSessionManager

Управляет жизненным циклом Guardian сессий:

```rust
pub struct GuardianReviewSessionManager {
    state: Arc<Mutex<GuardianReviewSessionState>>,
}

struct GuardianReviewSessionState {
    trunk: Option<Arc<GuardianReviewSession>>,           // Cached reusable session
    ephemeral_reviews: Vec<Arc<GuardianReviewSession>>, // Parallel reviews
}

impl GuardianReviewSessionManager {
    /// Run review, reusing trunk or creating ephemeral fork
    pub async fn run_review(&self, params: GuardianReviewSessionParams)
        -> GuardianReviewOutcome
    {
        // 1. Check trunk validity
        // 2. If trunk busy → create ephemeral
        // 3. If trunk invalid → create new trunk
        // 4. Run review on selected session
    }
}
```

### 4. GuardianReviewSession

Изолированная сессия Guardian агента:

```rust
pub struct GuardianReviewSession {
    codex: Codex,                              // The actual agent
    cancel_token: CancellationToken,
    reuse_key: GuardianReviewSessionReuseKey,  // Config fingerprint
    has_prior_review: AtomicBool,
    review_lock: Mutex<()>,
    last_committed_rollout_items: Mutex<Option<Vec<RolloutItem>>>,
}

impl GuardianReviewSession {
    /// Submit review request and wait for result
    pub async fn run_review_on_session(&self, params: GuardianReviewSessionParams)
        -> (GuardianReviewSessionOutcome, bool)
    {
        // 1. Submit Op::UserTurn with prompt
        // 2. Wait for TurnComplete
        // 3. Return outcome
    }
}
```

## Data Flow

```
1. Tool Call Request
        ↓
2. Sandbox Blocks (requires approval)
        ↓
3. routes_approval_to_guardian()? → YES
        ↓
4. Create/Use Guardian Session
        ↓
5. Build Review Request
   ┌────────────────────────────────────────┐
   │ • Transcript (history entries)         │
   │ • Action JSON (what to execute)        │
   │ • Retry reason (why retry needed)      │
   │ • JSON Schema (output format)           │
   └────────────────────────────────────────┘
        ↓
6. Submit Op::UserTurn to Guardian
        ↓
7. Guardian Agent Processes
   ┌────────────────────────────────────────┐
   │ • Reads policy.md                       │
   │ • Analyzes transcript                   │
   │ • Evaluates action risk                 │
   │ • May use read-only tools for evidence  │
   │ • Returns structured JSON assessment    │
   └────────────────────────────────────────┘
        ↓
8. Parse Assessment Result
        ↓
9. Decision:
   • Approved → retry tool with elevated sandbox
   • Denied → return ToolError::Rejected
   • Abort → wait for user
        ↓
10. Send Events to Main Session
    • GuardianAssessmentEvent (status)
    • WarningEvent (with rationale)
```

## Session Reuse Strategy

Guardian использует умную стратегию кэширования:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Session Selection Logic                      │
│                                                                 │
│  New Review Request                                             │
│        ↓                                                       │
│  Calculate reuse_key from config                                │
│        ↓                                                       │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ Trunk exists? → NO → Create new trunk session            │   │
│  └─────────────────────────────────────────────────────────┘   │
│        ↓                                                       │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ Trunk busy? → YES → Create ephemeral fork                │   │
│  │ Trunk valid? → NO  → Create new trunk                   │   │
│  │ Otherwise → Reuse trunk                                  │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  Trunk: Long-lived cached session (reused across reviews)       │
│  Ephemeral: Short-lived fork (destroyed after use)              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Reuse Key Components

Session reuse key includes:

- Model and provider
- Permissions (approval_policy, sandbox_policy)
- Developer/user instructions
- Compact prompt
- Working directory
- MCP servers
- Features flags

If any of these change → new trunk required.

## Guardian Agent Configuration

Guardian создаётся с изолированной конфигурацией:

```rust
fn build_guardian_review_session_config(parent_config: &Config) -> Config {
    Config {
        // Use preferred model
        model: GUARDIAN_PREFERRED_MODEL, // "gpt-5.4"

        // Guardian NEVER asks for approval (no recursion)
        permissions: Permissions {
            approval_policy: Constrained::allow_only(AskForApproval::Never),
            sandbox_policy: Constrained::allow_only(SandboxPolicy::new_read_only_policy()),
            ..Default::default()
        },

        // No network modifications
        network_proxy: parent_config.network_proxy.clone(),

        // Disable dangerous features
        features: Features {
            spawn_csv: false,
            collab: false,
            web_search_request: false,
            web_search_cached: false,
            ..Default::default()
        },

        // Use policy.md as instructions
        developer_instructions: policy.md content,
    }
}
```

## Event Flow

```
Main Session                          Guardian Session
     │                                      │
     │  ──── routes_approval_to_guardian ──▶│
     │                                      │
     │                                      │── Build prompt
     │                                      │── Submit Op::UserTurn
     │◀─── GuardianAssessment(InProgress) ──│
     │                                      │
     │                                      │── Guardian processes
     │                                      │── Returns JSON assessment
     │◀─── GuardianAssessment(Approved/Denied)│
     │◀─── WarningEvent (with rationale) ───│
     │                                      │
     ▼                                      ▼
```

## File Structure

```
core/src/guardian/
├── mod.rs                    # Module exports, constants
├── review.rs                 # Main review logic (run_guardian_review)
├── review_session.rs        # Session management (trunk, ephemeral)
├── approval_request.rs       # Request type definitions
├── prompt.rs                 # Prompt building and parsing
└── policy.md                 # Risk assessment guidelines

core/src/
├── tools/orchestrator.rs     # Orchestrator with guardian integration
├── guardian/
│   ├── mod.rs
│   ├── review.rs
│   ├── review_session.rs
│   ├── approval_request.rs
│   ├── prompt.rs
│   └── policy.md
└── protocol/src/approvals.rs # Protocol types
```
