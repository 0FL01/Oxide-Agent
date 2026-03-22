# Guardian Review Flow

Полный жизненный цикл Guardian review от запроса до решения.

## High-Level Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           PHASE 1: Trigger                            │
│                                                                         │
│  Main Agent → Tool → Sandbox blocks → Need approval                     │
│                                   ↓                                     │
│                    routes_approval_to_guardian()?                        │
│                         ↓           ↓                                   │
│                      YES          NO → User approval                    │
│                         ↓                                              │
└─────────────────────────┼───────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     PHASE 2: Session Acquisition                       │
│                                                                         │
│  GuardianReviewSessionManager.run_review()                               │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Session Selection Strategy                            │  │
│  │                                                                   │  │
│  │  1. Calculate reuse_key from current config                       │  │
│  │  2. Check trunk validity                                          │  │
│  │     • Trunk exists? → continue                                    │  │
│  │     • Reuse key matches? → reuse trunk                             │  │
│  │     • Trunk busy? → create ephemeral fork                         │  │
│  │     • Key mismatch? → shutdown old, create new                     │  │
│  │                                                                   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  Selected: trunk or ephemeral session                                   │
│                                                                         │
└─────────────────────────┬───────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                       PHASE 3: Prompt Building                         │
│                                                                         │
│  build_guardian_prompt_items()                                          │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                    Prompt Structure                               │  │
│  │                                                                   │  │
│  │  1. System instruction (from policy.md)                          │  │
│  │  2. Transcript (bounded by token limits)                         │  │
│  │     • Recent user messages (always included)                      │  │
│  │     • Recent assistant messages (last 40)                         │  │
│  │     • Tool calls and results                                      │  │
│  │     • Truncated if exceeds GUARDIAN_MAX_*_TRANSCRIPT_TOKENS       │  │
│  │  3. Approval request block                                        │  │
│  │     • Retry reason (if applicable)                                 │  │
│  │     • Action JSON (formatted, truncated if > 1000 tokens)          │  │
│  │  4. Output schema (strict JSON contract)                          │  │
│  │                                                                   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  Vector of UserInput items                                              │
│                                                                         │
└─────────────────────────┬───────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        PHASE 4: Review Execution                        │
│                                                                         │
│  run_review_on_session()                                               │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Op::UserTurn Submission                             │  │
│  │                                                                   │  │
│  │  review_session.codex.submit(Op::UserTurn {                      │  │
│  │      items: prompt_items,                                         │  │
│  │      approval_policy: AskForApproval::Never,                       │  │
│  │      sandbox_policy: SandboxPolicy::new_read_only_policy(),       │  │
│  │      final_output_json_schema: Some(schema),                      │  │
│  │      // ... other fields                                          │  │
│  │  })                                                               │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Wait for Result                                      │  │
│  │                                                                   │  │
│  │  tokio::select! {                                                 │  │
│  │      // 1. Timeout (90 seconds)                                   │  │
│  │      // 2. External cancel                                        │  │
│  │      // 3. EventMsg::TurnComplete                                 │  │
│  │      // 4. EventMsg::TurnAborted                                  │  │
│  │  }                                                                │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  GuardianReviewSessionOutcome                                           │
│                                                                         │
└─────────────────────────┬───────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                         PHASE 5: Decision                             │
│                                                                         │
│  run_guardian_review() processes outcome                               │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Outcome → Assessment Mapping                         │  │
│  │                                                                   │  │
│  │  Completed(Ok(assessment))  → use assessment                    │  │
│  │  Completed(Err(e))          → High(100): "review failed: {e}"   │  │
│  │  TimedOut                    → High(100): "timed out"           │  │
│  │  Aborted                     → ReviewDecision::Abort             │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Final Decision                                        │  │
│  │                                                                   │  │
│  │  risk_score < 80  →  ReviewDecision::Approved                    │  │
│  │  risk_score >= 80  →  ReviewDecision::Denied                      │  │
│  │                                                                   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                   ↓                                     │
│  Events sent to main session:                                          │
│  • GuardianAssessmentEvent (status: Approved/Denied)                  │
│  • WarningEvent (with rationale and risk details)                      │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     PHASE 6: Orchestrator Response                     │
│                                                                         │
│  ToolOrchestrator receives ReviewDecision                              │
│                                   ↓                                     │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │              Decision Handling                                    │  │
│  │                                                                   │  │
│  │  ReviewDecision::Approved                                         │  │
│  │      → Retry tool with elevated sandbox (SandboxType::None)      │  │
│  │                                                                   │  │
│  │  ReviewDecision::Denied                                           │  │
│  │      → Return ToolError::Rejected                                 │  │
│  │      → Tool call does NOT execute                                 │  │
│  │                                                                   │  │
│  │  ReviewDecision::Abort                                            │  │
│  │      → Wait for user intervention                                  │  │
│  │                                                                   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

## Detailed Step-by-Step

### Step 1: Routing Check

```rust
// In orchestrator.rs
pub fn routes_approval_to_guardian(turn: &TurnContext) -> bool {
    turn.approval_policy.value() == AskForApproval::OnRequest
        && turn.config.approvals_reviewer == ApprovalsReviewer::GuardianSubagent
}
```

Both conditions must be true.

### Step 2: Session Acquisition

```rust
// In review_session.rs - GuardianReviewSessionManager::run_review()

// 1. Calculate reuse key
let next_reuse_key = calculate_reuse_key(&params);

// 2. Check existing trunk
if let Some(trunk) = &state.trunk {
    if trunk.reuse_key == next_reuse_key && trunk.try_lock().is_ok() {
        // Reuse trunk
        return run_review_on_trunk(trunk, params).await;
    }
}

// 3. Create new trunk or ephemeral
if trunk.is_none() || trunk.stale() {
    let new_trunk = spawn_guardian_review_session(...).await?;
    state.trunk = Some(new_trunk);
    return run_review_on_trunk(new_trunk, params).await;
} else {
    // Trunk busy, create ephemeral
    return run_ephemeral_review(trunk, params).await;
}
```

### Step 3: Prompt Building

```rust
// In prompt.rs - build_guardian_prompt_items()

pub fn build_guardian_prompt_items(
    request: &GuardianApprovalRequest,
    transcript: &[ResponseItem],
    retry_reason: Option<&str>,
    schema: &Value,
) -> Result<Vec<UserInput>> {

    // 1. Collect transcript entries with token budgets
    let transcript_entries = collect_guardian_transcript_entries(
        transcript,
        GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS,
        GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS,
        GUARDIAN_RECENT_ENTRY_LIMIT,
    );

    // 2. Format action as JSON
    let action_json = format_guardian_action_pretty(request)?;

    // 3. Build instruction string
    let instruction = format!(
        "The following is the Codex agent history...\n\
         >>> TRANSCRIPT START\n\
         {transcript}\n\
         >>> TRANSCRIPT END\n\
         \n\
         The Codex agent has requested the following action:\n\
         >>> APPROVAL REQUEST START\n\
         {retry_reason}\n\
         Planned action JSON:\n\
         {action_json}\n\
         >>> APPROVAL REQUEST END\n\
         \n\
         Output schema: {schema}"
    );

    Ok(vec![UserInput::Message { role: "user", content: instruction }])
}
```

### Step 4: Transcript Collection

```rust
// In prompt.rs - collect_guardian_transcript_entries()

enum GuardianTranscriptEntryKind {
    User,
    Assistant,
    Tool(String),  // tool name
}

struct GuardianTranscriptEntry {
    kind: GuardianTranscriptEntryKind,
    text: String,
}

fn collect_guardian_transcript_entries(
    items: &[ResponseItem],
    max_message_tokens: usize,
    max_tool_tokens: usize,
    max_entries: usize,
) -> Vec<GuardianTranscriptEntry> {
    let mut entries = Vec::new();
    let mut message_tokens = 0;
    let mut tool_tokens = 0;

    for item in items.iter().rev().take(max_entries) {
        match item {
            ResponseItem::Message { role: "user", content } => {
                // Always include user messages
                let entry = GuardianTranscriptEntry {
                    kind: GuardianTranscriptEntryKind::User,
                    text: content.to_string(),
                };
                entries.push(entry);
            }
            ResponseItem::Message { role: "assistant", content } => {
                // Include with token budget
                let tokens = estimate_tokens(content);
                if message_tokens + tokens <= max_message_tokens {
                    message_tokens += tokens;
                    entries.push(GuardianTranscriptEntry {
                        kind: GuardianTranscriptEntryKind::Assistant,
                        text: content.to_string(),
                    });
                }
            }
            ResponseItem::FunctionCall { name, arguments, .. } => {
                let tokens = estimate_tokens(arguments);
                if tool_tokens + tokens <= max_tool_tokens {
                    tool_tokens += tokens;
                    entries.push(GuardianTranscriptEntry {
                        kind: GuardianTranscriptEntryKind::Tool(name.clone()),
                        text: serde_json::to_string(arguments).unwrap_or_default(),
                    });
                }
            }
            // ... other variants
        }
    }

    // Truncate if over budget
    if message_tokens > max_message_tokens || tool_tokens > max_tool_tokens {
        insert_truncation_markers(&mut entries);
    }

    entries
}
```

### Step 5: Review Execution

```rust
// In review_session.rs - run_review_on_session()

async fn run_review_on_session(
    session: &GuardianReviewSession,
    params: GuardianReviewSessionParams,
) -> (GuardianReviewSessionOutcome, bool) {

    // 1. If prior review exists, append reminder
    if session.has_prior_review.load(Ordering::Relaxed) {
        append_guardian_followup_reminder(&session.codex).await;
    }

    // 2. Sync network approvals
    sync_session_approved_hosts_to(&session.codex, &params).await;

    // 3. Submit for review
    let submission_id = session.codex.submit(Op::UserTurn {
        items: params.prompt_items,
        approval_policy: AskForApproval::Never,
        sandbox_policy: SandboxPolicy::new_read_only_policy(),
        final_output_json_schema: Some(params.schema),
        // ...
    }).await?;

    // 4. Wait for result with timeout
    wait_for_guardian_review(
        session,
        params.external_cancel,
        deadline,
    ).await
}
```

### Step 6: Wait Loop

```rust
// In review_session.rs - wait_for_guardian_review()

async fn wait_for_guardian_review(
    session: &GuardianReviewSession,
    external_cancel: Option<CancellationToken>,
    deadline: Instant,
) -> (GuardianReviewSessionOutcome, bool) {

    loop {
        tokio::select! {
            // 1. Timeout
            _ = tokio::time::sleep_until(deadline) => {
                interrupt_and_drain_turn(session).await;
                return (GuardianReviewSessionOutcome::TimedOut, true);
            }

            // 2. External cancel
            _ = async {
                if let Some(token) = external_cancel {
                    token.cancelled().await;
                }
            } => {
                interrupt_and_drain_turn(session).await;
                return (GuardianReviewSessionOutcome::Aborted, true);
            }

            // 3. Event from Codex
            event = session.codex.next_event() => {
                match event {
                    EventMsg::TurnComplete { last_agent_message } => {
                        session.has_prior_review.store(true, Ordering::Relaxed);
                        return (
                            GuardianReviewSessionOutcome::Completed(Ok(last_agent_message)),
                            true,
                        );
                    }
                    EventMsg::TurnAborted => {
                        return (GuardianReviewSessionOutcome::Aborted, true);
                    }
                    _ => continue,  // Ignore other events
                }
            }
        }
    }
}
```

### Step 7: Result Parsing

```rust
// In prompt.rs - parse_guardian_assessment()

pub fn parse_guardian_assessment(
    text: Option<&str>,
) -> anyhow::Result<GuardianAssessment> {

    let text = text.context("guardian review completed without an assessment payload")?;

    // Try direct parse first
    if let Ok(assessment) = serde_json::from_str::<GuardianAssessment>(text) {
        return Ok(assessment);
    }

    // Try finding JSON in text (model might wrap in prose)
    if let Some(json_start) = text.find('{') {
        if let Some(json_end) = text.rfind('}') {
            let json_str = &text[json_start..=json_end];
            if let Ok(assessment) = serde_json::from_str::<GuardianAssessment>(json_str) {
                return Ok(assessment);
            }
        }
    }

    anyhow::bail!("guardian assessment was not valid JSON")
}

#[derive(Deserialize)]
struct GuardianAssessment {
    risk_level: GuardianRiskLevel,   // "low" | "medium" | "high"
    risk_score: u8,                    // 0-100
    rationale: String,
    evidence: Vec<GuardianEvidence>,
}

struct GuardianEvidence {
    message: String,
    why: String,
}
```

### Step 8: Final Decision

```rust
// In review.rs - run_guardian_review()

pub async fn run_guardian_review(
    session: &Session,
    request: GuardianApprovalRequest,
    // ...
) -> ReviewDecision {

    // 1. Send InProgress event
    session.send_event(EventMsg::GuardianAssessment(
        GuardianAssessmentEvent {
            status: GuardianAssessmentStatus::InProgress,
            // ...
        }
    )).await;

    // 2. Check external cancel
    if external_cancel.is_cancelled() {
        return ReviewDecision::Abort;
    }

    // 3. Build prompt
    let prompt_items = build_guardian_prompt_items(...)?;

    // 4. Run review on session
    let (outcome, keep_session) = session.guardian_review_session.run_review(params).await;

    // 5. Map outcome to assessment
    let assessment = match outcome {
        GuardianReviewSessionOutcome::Completed(Ok(text)) => {
            parse_guardian_assessment(text.as_deref())?
        }
        GuardianReviewSessionOutcome::Completed(Err(e)) => {
            GuardianAssessment {
                risk_level: GuardianRiskLevel::High,
                risk_score: 100,
                rationale: format!("Automatic approval review failed: {e}"),
                evidence: vec![],
            }
        }
        GuardianReviewSessionOutcome::TimedOut => {
            GuardianAssessment {
                risk_level: GuardianRiskLevel::High,
                risk_score: 100,
                rationale: "Automatic approval review timed out after 90 seconds".into(),
                evidence: vec![],
            }
        }
        GuardianReviewSessionOutcome::Aborted => {
            return ReviewDecision::Abort;
        }
    };

    // 6. Make decision
    let approved = assessment.risk_score < GUARDIAN_APPROVAL_RISK_THRESHOLD;

    // 7. Send warning event
    if !approved {
        session.send_event(warning_event_denied(&assessment)).await;
    }

    // 8. Send final event
    session.send_event(EventMsg::GuardianAssessment(
        GuardianAssessmentEvent {
            status: if approved { Approved } else { Denied },
            risk_score: Some(assessment.risk_score),
            risk_level: Some(assessment.risk_level),
            rationale: Some(assessment.rationale),
            // ...
        }
    )).await;

    // 9. Return decision
    if approved {
        ReviewDecision::Approved
    } else {
        ReviewDecision::Denied
    }
}
```

## Token Budgets

Guardian использует строгие лимиты для защиты от контекстных атак:

| Budget                                   | Limit  | Purpose                         |
| ---------------------------------------- | ------ | ------------------------------- |
| `GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS` | 10,000 | User/assistant messages         |
| `GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS`    | 10,000 | Tool calls/results              |
| `GUARDIAN_MAX_MESSAGE_ENTRY_TOKENS`      | 2,000  | Per message entry               |
| `GUARDIAN_MAX_TOOL_ENTRY_TOKENS`         | 1,000  | Per tool entry                  |
| `GUARDIAN_MAX_ACTION_STRING_TOKENS`      | 1,000  | Action JSON string              |
| `GUARDIAN_RECENT_ENTRY_LIMIT`            | 40     | Max entries from recent history |

## Error Handling

| Error Type           | Behavior    | Risk Score |
| -------------------- | ----------- | ---------- |
| Parse failure        | Fail-closed | 100 (High) |
| Build prompt failure | Fail-closed | 100 (High) |
| Timeout (90s)        | Fail-closed | 100 (High) |
| External cancel      | Abort       | N/A        |
| Guardian crash       | Fail-closed | 100 (High) |

## Truncation Strategy

When transcript exceeds token budgets:

```
Original: [A] [B] [C] [D] [E] [F] [G] [H]

After prefix+suffix truncation:
       [A] [B] ...<truncated omitted_approx_tokens="N"/>... [G] [H]
```

- Keep recent entries (prefix)
- Keep oldest entries (suffix)
- Insert marker in middle
- Marker indicates approximate tokens omitted
