# Guardian Quick Reference

Quick lookup guide for Guardian implementation.

## Configuration

```rust
// Enable Guardian
config.approvals_reviewer = ApprovalsReviewer::GuardianSubagent;
config.approval_policy = AskForApproval::OnRequest;

// Guardian settings
guardian.model = "gpt-5.4";
guardian.timeout = 90.seconds();
guardian.risk_threshold = 80;
```

## Constants

| Constant                                 | Value       | Description              |
| ---------------------------------------- | ----------- | ------------------------ |
| `GUARDIAN_PREFERRED_MODEL`               | `"gpt-5.4"` | Default model            |
| `GUARDIAN_REVIEW_TIMEOUT`                | `90s`       | Max review duration      |
| `GUARDIAN_APPROVAL_RISK_THRESHOLD`       | `80`        | Score ≥ 80 = Denied      |
| `GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS` | `10,000`    | Message token budget     |
| `GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS`    | `10,000`    | Tool token budget        |
| `GUARDIAN_RECENT_ENTRY_LIMIT`            | `40`        | Max entries from history |

## Risk Score Ranges

| Range  | Level  | Action                   |
| ------ | ------ | ------------------------ |
| 0-39   | Low    | Approved                 |
| 40-79  | Medium | Approved (review passed) |
| 80-100 | High   | Denied                   |

## Request Types

```rust
GuardianApprovalRequest::Shell {
    id, command, cwd, justification
}

GuardianApprovalRequest::NetworkAccess {
    id, target, host, port
}

GuardianApprovalRequest::FileWrite {
    id, path, content_preview
}
```

## Decision Types

```rust
ReviewDecision::Approved   // Execute the command
ReviewDecision::Denied     // Block the command
ReviewDecision::Abort      // Wait for user
```

## Workflow

```
1. Tool blocked by sandbox
        ↓
2. routes_approval_to_guardian()? → YES
        ↓
3. Get/create Guardian session
        ↓
4. Build prompt with transcript + action
        ↓
5. Call model with JSON schema output
        ↓
6. Parse response → risk_score
        ↓
7. risk_score < 80? → Approved → Retry
                         ↓ No
                    → Denied → Error
```

## Token Budget Allocation

```
┌─────────────────────────────────────────────────────┐
│           10,000 tokens (messages)                 │
│  ┌──────────────────────────────────────────────┐  │
│  │  User messages (always included)              │  │
│  │  Recent assistant messages (≤40 entries)      │  │
│  └──────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│            10,000 tokens (tools)                   │
│  ┌──────────────────────────────────────────────┐  │
│  │  Tool calls and results                      │  │
│  │  Max 1,000 tokens per entry                  │  │
│  └──────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│            1,000 tokens (action JSON)               │
│  ┌──────────────────────────────────────────────┐  │
│  │  Action details                               │  │
│  └──────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

## JSON Output Schema

```json
{
  "risk_level": "low|medium|high",
  "risk_score": 0-100,
  "rationale": "Why this risk level",
  "evidence": [
    {"message": "fact", "why": "significance"}
  ]
}
```

## Session Reuse Key

Fields that trigger new session:

- Model / Provider
- Permissions (approval_policy, sandbox_policy)
- Developer/User instructions (hashed)
- Features flags
- MCP servers

Fields that DON'T trigger new session:

- Working directory
- Timestamps
- Session ID

## Event Flow

```
Main Session                          Guardian
     │                                    │
     │─── routes_approval_to_guardian ───▶│
     │                                    │
     │◀─── GuardianAssessment(InProgress) │
     │                                    │
     │◀─── GuardianAssessment(Approved)   │
     │◀─── WarningEvent (if denied)       │
     │                                    │
     ▼                                    ▼
```

## Quick Decision Checklist

For `risk_score >= 80` (HIGH):

- [ ] Deletes/corrupts important data?
- [ ] Disrupts services?
- [ ] Changes permissions?
- [ ] Exposes secrets?
- [ ] Sends data to untrusted destination?
- [ ] Probes for credentials?
- [ ] Clear destructive impact?

For `risk_score < 80` (LOW/MEDIUM):

- [ ] Routine operation?
- [ ] User explicitly requested?
- [ ] Narrowly scoped?
- [ ] Read-only or reversible?
- [ ] Benign filesystem action?

## Error Handling

| Error             | Action      | Score |
| ----------------- | ----------- | ----- |
| Parse failure     | Fail-closed | 100   |
| Timeout (90s)     | Fail-closed | 100   |
| Build prompt fail | Fail-closed | 100   |
| External cancel   | Abort       | N/A   |

## Minimal Implementation

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Guardian {
    model_api: fn(String) -> anyhow::Result<String>,
    threshold: u8,
}

#[derive(Serialize, Deserialize)]
struct Assessment {
    risk_level: String,
    risk_score: u8,
    rationale: String,
}

impl Guardian {
    pub fn review(&self, action: &str, context: &str) -> anyhow::Result<bool> {
        let prompt = format!(
            "Policy: {}\nContext: {}\nAction: {}\nRespond JSON",
            include_str!("policy.md"),
            context,
            action
        );

        let response = (self.model_api)(prompt)?;

        let a: Assessment = serde_json::from_str(&response)
            .or_else(|_| {
                let start = response.find('{')?;
                let end = response.rfind('}')? + 1;
                serde_json::from_str(&response[start..end])
            })?;

        Ok(a.risk_score < self.threshold)
    }
}
```

## File Locations

```
codex-rs/
├── core/src/
│   └── guardian/
│       ├── mod.rs              # Exports, constants
│       ├── review.rs           # Main review logic
│       ├── review_session.rs   # Session management
│       ├── approval_request.rs # Request types
│       ├── prompt.rs           # Prompt building
│       └── policy.md           # Risk policy
└── docs/guard/
    ├── index.md                # Overview
    ├── architecture.md         # Architecture
    ├── flow.md                 # Flow diagrams
    ├── policy.md               # Full policy
    ├── api.md                  # API reference
    ├── implementation-guide.md # Porting guide
    ├── troubleshooting.md      # Common issues
    └── quick-reference.md      # This file
```
