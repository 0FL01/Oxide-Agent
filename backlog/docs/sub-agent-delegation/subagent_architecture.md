# Sub-Agent Architecture Documentation

## Overview

This document describes the sub-agent architecture used in OpenCode, designed to enable specialized AI assistants to handle specific tasks within a coding workflow. The system allows primary agents to spawn child sessions with dedicated sub-agents, providing task isolation, permission management, and session hierarchy.

## Core Concepts

### Agent Types

There are three agent modes defined in the system:

1. **primary** - Main assistants that users interact with directly
2. **subagent** - Specialized assistants invoked by primary agents for specific tasks
3. **all** - Universal agents that can function in both roles

### Session Hierarchy

```
Parent Session (Primary Agent)
├── Child Session 1 (Subagent: explore)
├── Child Session 2 (Subagent: general)
└── Child Session 3 (Subagent: custom-agent)
    └── Nested Child Session (Subagent: another-agent)
```

Each child session has a `parentID` pointing to its parent, enabling navigation and result aggregation.

## Task Tool Implementation

The primary mechanism for invoking sub-agents is the Task Tool.

### Tool Definition

```typescript
const TaskTool = Tool.define("task", async () => {
  const agents = await Agent.list().then((x) => x.filter((a) => a.mode !== "primary"))

  return {
    description: DESCRIPTION.replace(
      "{agents}",
      agents.map((a) => `- ${a.name}: ${a.description ?? "Manual use only"}`).join("\n"),
    ),
    parameters: z.object({
      description: z.string().describe("Short (3-5 words) task description"),
      prompt: z.string().describe("Detailed task for the agent"),
      subagent_type: z.string().describe("Agent type to use"),
      session_id: z.string().describe("Existing session to continue").optional(),
      command: z.string().describe("Command that triggered this task").optional(),
    }),
  }
})
```

### Execution Flow

1. **Permission Check**: Verify the primary agent has permission to spawn sub-agents
2. **Agent Lookup**: Retrieve the sub-agent configuration
3. **Session Creation**: Create a new child session with restricted permissions
4. **Model Selection**: Use sub-agent's model or inherit from parent
5. **Prompt Execution**: Run the sub-agent with specified tools
6. **Result Aggregation**: Collect tool execution results and return summary
7. **Cleanup**: Unsubscribe from event listeners

### Permission Management

Child sessions have restricted access to prevent recursion and unauthorized actions:

```typescript
const session = await Session.create({
  parentID: ctx.sessionID,
  title: `${description} (@${agent.name} subagent)`,
  permission: [
    { permission: "todowrite", pattern: "*", action: "deny" },
    { permission: "todoread", pattern: "*", action: "deny" },
    { permission: "task", pattern: "*", action: "deny" },
    // Optional: Allow specific primary tools
    ...(config.experimental?.primary_tools?.map((t) => ({
      pattern: "*",
      action: "allow",
      permission: t,
    })) ?? []),
  ],
})
```

### Event-Based Progress Tracking

The Task Tool subscribes to `MessageV2.Event.PartUpdated` to track real-time progress:

```typescript
const parts: Record<string, { id: string; tool: string; state: any }> = {}
const unsub = Bus.subscribe(MessageV2.Event.PartUpdated, async (evt) => {
  if (evt.properties.part.sessionID !== session.id) return
  if (evt.properties.part.messageID === messageID) return
  if (evt.properties.part.type !== "tool") return

  const part = evt.properties.part
  parts[part.id] = {
    id: part.id,
    tool: part.tool,
    state: {
      status: part.state.status,
      title: part.state.status === "completed" ? part.state.title : undefined,
    },
  }

  // Update parent with progress metadata
  ctx.metadata({
    title: params.description,
    metadata: {
      summary: Object.values(parts).sort((a, b) => a.id.localeCompare(b.id)),
      sessionId: session.id,
    },
  })
})
```

## Built-in Sub-Agents

### General Agent

```typescript
{
  name: "general",
  description: "General-purpose agent for researching complex questions and executing multi-step tasks",
  mode: "subagent",
  permission: {
    todowrite: "deny",
    todoread: "deny"
  }
}
```

Use cases:

- Complex, multi-step research tasks
- Parallel execution of work units
- Tasks requiring multiple tool invocations

### Explore Agent

```typescript
{
  name: "explore",
  description: "Fast agent specialized for exploring codebases. Use to quickly find files, search code keywords, or answer codebase questions",
  mode: "subagent",
  permission: {
    grep: "allow",
    glob: "allow",
    list: "allow",
    bash: "allow",
    webfetch: "allow",
    websearch: "allow",
    codesearch: "allow",
    read: "allow"
  }
}
```

Use cases:

- Finding files by patterns (`src/components/**/*.tsx`)
- Searching for keywords in code
- Understanding codebase architecture
- Answering "how does X work?" questions

Thoroughness levels: `"quick"` | `"medium"` | `"very thorough"`

## Agent Configuration

### JSON Configuration (`opencode.json`)

```json
{
  "agent": {
    "code-reviewer": {
      "mode": "subagent",
      "description": "Reviews code for best practices and potential issues",
      "model": "anthropic/claude-sonnet-4-20250514",
      "permission": {
        "edit": "deny",
        "write": "deny"
      },
      "prompt": "You are a code reviewer. Focus on security, performance, and maintainability."
    }
  }
}
```

### Markdown Configuration (`~/.config/opencode/agent/review.md`)

```markdown
---
description: Reviews code for quality and best practices
mode: subagent
model: anthropic/claude-sonnet-4-20250514
temperature: 0.1
---

You are in code review mode. Focus on:

- Code quality and best practices
- Potential bugs and edge cases
- Performance implications
- Security considerations

Provide constructive feedback without making direct changes.
```

### Agent Options

- `mode`: `"primary"` | `"subagent"` | `"all"`
- `model`: Provider and model identifier
- `temperature`: Sampling temperature (0.0-1.0)
- `topP`: Nucleus sampling parameter
- `prompt`: System prompt override
- `description`: Brief description for LLM awareness
- `permission`: Permission ruleset
- `color`: UI color for agent
- `options`: Custom options dict
- `steps`: Maximum reasoning steps (if applicable)

## Session Navigation

Users can navigate between parent and child sessions:

```
<Leader>+Right  : parent → child1 → child2 → ... → parent
<Leader>+Left   : parent ← child1 ← child2 ← ... → parent
```

Keybindings:

- `switch_agent`: Cycle through primary agents (default: Tab)
- `session_child_cycle`: Cycle through child sessions forward
- `session_child_cycle_reverse`: Cycle through child sessions backward

## Command Integration

Commands can be configured to trigger subagent execution:

```typescript
{
  name: "check-file",
  agent: "explore",  // If subagent, triggers subtask execution
  description: "Check a file for issues",
  subtask: true,  // Force subagent behavior
  command: "/check-file path/to/file.py"
}
```

### Subtask Detection Logic

```typescript
// If agent is subagent and command.subtask !== false, OR if command.subtask === true
const isSubtask = (agent.mode === "subagent" && command.subtask !== false) || command.subtask === true

if (isSubtask) {
  // Transform command into Task tool invocation
  parts = [
    {
      type: "subtask",
      agent: agent.name,
      description: command.description ?? "",
      prompt: templateParts.find((y) => y.type === "text")?.text ?? "",
    },
  ]
}
```

## Usage Patterns

### Pattern 1: Automatic Subagent Selection

Primary agent automatically calls subagents based on task descriptions:

```
User: "Help me understand how authentication works in this codebase"

Primary Agent:
→ Calls @explore subagent with prompt "Analyze authentication flow"
→ Receives analysis
→ Presents findings to user
```

### Pattern 2: Manual @ Mention

User can explicitly invoke subagents:

```
User: "@general refactor this code to be more efficient"

Primary Agent:
→ Calls @general subagent
→ General handles the refactoring
→ Returns optimized code
```

### Pattern 3: Parallel Task Execution

Execute multiple subagents concurrently:

```
Primary Agent:
→ Calls @explore (thoroughness: "quick") to find all API endpoints
→ Calls @explore (thoroughness: "medium") to understand data flow
→ Calls @general to analyze patterns
→ Awaits all results
→ Synthesizes comprehensive response
```

### Pattern 4: Nested Subtasks

Subagents can call other subagents:

```
Parent (build)
└── Child 1 (general)
    └── Grandchild (explore)
    └── Grandchild (custom-agent)
```

## Rust Implementation Considerations

For implementing this architecture in Rust:

### Data Structures

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub name: String,
    pub mode: AgentMode,
    pub description: Option<String>,
    pub permission: PermissionRuleset,
    pub model: Option<ModelConfig>,
    pub prompt: Option<String>,
    pub options: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub project_id: String,
    pub title: String,
    pub permission: Option<PermissionRuleset>,
    // ... other fields
}
```

### Task Tool Execution Flow

```rust
pub async fn execute_task(
    params: TaskParams,
    ctx: ToolContext,
) -> Result<TaskResult, Error> {
    // 1. Check permissions
    check_permission(&ctx, "task", &params.subagent_type).await?;

    // 2. Get agent configuration
    let agent = Agent::get(&params.subagent_type)
        .ok_or_else(|| Error::AgentNotFound(params.subagent_type.clone()))?;

    // 3. Create child session
    let session = Session::create(SessionCreateParams {
        parent_id: Some(ctx.session_id.clone()),
        title: format!("{} (@{} subagent)", params.description, agent.name),
        permission: vec![
            PermissionRule {
                permission: "todowrite".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Deny,
            },
            // ... more restrictions
        ],
        ..Default::default()
    }).await?;

    // 4. Execute prompt in child session
    let result = execute_prompt(session.id, &params.prompt, &agent).await?;

    // 5. Return summary to parent
    Ok(TaskResult {
        title: params.description,
        metadata: TaskMetadata {
            session_id: session.id,
            summary: result.tool_calls,
        },
        output: result.text,
    })
}
```

### Event Bus Pattern

Use a tokio broadcast channel for event streaming:

```rust
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub enum SessionEvent {
    MessageCreated { session_id: String, message: Message },
    PartUpdated { session_id: String, message_id: String, part: Part },
    SessionUpdated { info: SessionInfo },
}

pub struct EventBus {
    sender: broadcast::Sender<SessionEvent>,
}

impl EventBus {
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: SessionEvent) {
        let _ = self.sender.send(event);
    }
}
```

### Permission System

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    pub action: PermissionAction,
}

pub fn check_permission(
    ruleset: &[PermissionRule],
    permission: &str,
    pattern: &str,
) -> Result<bool, PermissionError> {
    // Match pattern against rules
    // Return specific action or ask user
    todo!()
}
```

## Best Practices

### For Primary Agents

1. **Use subagents for**: Multi-step tasks, codebase exploration, parallel work
2. **Avoid subagents for**: Single file reads, simple operations, user communication
3. **Always summarize**: Subagent results are not visible to users - provide context

### For Subagent Developers

1. **Clear descriptions**: Describe when to use the subagent in the description field
2. **Restrictive permissions**: Default to denying access to dangerous operations
3. **Focused scope**: Each subagent should have a single, well-defined purpose
4. **Model selection**: Specify appropriate model for the task

### For Users

1. **@ mention control**: Use `@agent-name` to manually invoke subagents
2. **Session navigation**: Use keybindings to inspect subagent sessions
3. **Custom agents**: Create project-specific subagents for common workflows

## File Structure Reference

```
packages/opencode/src/
├── agent/
│   └── agent.ts              # Agent registry and built-in agents
├── tool/
│   ├── task.ts              # Task tool implementation
│   └── task.txt             # Task tool description for LLMs
├── session/
│   ├── index.ts             # Session management
│   ├── prompt.ts            # Session prompt processing (lines 351-376 for subtasks)
│   └── message-v2.ts        # Message and part definitions
├── permission/
│   └── next.ts              # Permission system
└── config/
    └── config.ts            # Configuration loading
```

## Key Takeaways for LLM Coders

1. **Sub-agents provide task isolation**: Each subagent runs in its own session with restricted permissions
2. **Parent-child hierarchy**: Results flow up the session tree; child sessions don't interact with users
3. **Permission-based security**: Subagents inherit and modify permission rulesets
4. **Event-driven updates**: Task tools subscribe to part update events to track progress
5. **Model flexibility**: Subagents can use their own models or inherit from parent
6. **Customizable**: Users can define project-specific subagents via JSON or Markdown files
7. **Tool restriction**: Child sessions are denied access to certain tools (todowrite, todoread, task) to prevent recursion
8. **Navigation support**: Users can inspect and navigate between parent and child sessions

This architecture enables sophisticated multi-agent workflows while maintaining security, isolation, and user control.
