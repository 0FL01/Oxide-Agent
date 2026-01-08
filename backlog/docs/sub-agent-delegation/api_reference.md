# Sub-Agent API Quick Reference

## Agent Management

### Get Agent

```typescript
const agent = await Agent.get(agentName)
```

Returns `Agent.Info` or `null`

### List Agents

```typescript
const agents = await Agent.list()
```

Returns `Agent.Info[]` sorted by default agent first

### List Subagents Only

```typescript
const subagents = await Agent.list().then((agents) => agents.filter((a) => a.mode !== "primary" && !a.hidden))
```

### Agent Info Structure

```typescript
interface AgentInfo {
  name: string
  description?: string
  mode: "primary" | "subagent" | "all"
  native?: boolean
  hidden?: boolean
  topP?: number
  temperature?: number
  color?: string
  permission: PermissionRuleset
  model?: {
    modelID: string
    providerID: string
  }
  prompt?: string
  options: Record<string, any>
  steps?: number
}
```

## Session Management

### Create Session

```typescript
const session = await Session.create({
  parentID: parentId, // Optional: creates child session if provided
  projectID: projectId,
  title: sessionTitle,
  permission: [
    { permission: "tool-name", pattern: "*", action: "deny" },
    // ... more rules
  ],
})
```

### Get Session

```typescript
const session = await Session.get(sessionId)
```

### Get Messages

```typescript
const messages = await Session.messages({ sessionID: sessionId })
```

### Update Session

```typescript
await Session.update(sessionId, { title: newTitle })
```

### Navigate Sessions

```typescript
// Get children
const children = await Session.children(parentId)

// Get parent
const parent = await Session.parent(childId)

// Navigate to next (child or parent)
const next = await Session.navigateNext(currentId)

// Navigate to previous (parent)
const previous = await Session.navigatePrevious(currentId)
```

### Session Info Structure

```typescript
interface SessionInfo {
  id: string
  parentID?: string
  projectID: string
  directory: string
  title: string
  permission?: PermissionRuleset
  time: {
    created: number
    updated: number
    compacting?: number
    archived?: number
  }
  summary?: {
    additions: number
    deletions: number
    files: number
    diffs?: FileDiff[]
  }
  share?: {
    url: string
  }
  revert?: {
    messageID: string
    partID?: string
    snapshot?: string
    diff?: string
  }
}
```

## Task Tool

### Execute Task

```typescript
const result = await TaskTool.execute(
  {
    description: "Short task description (3-5 words)",
    prompt: "Detailed task instructions",
    subagent_type: "agent-name",
    session_id: "existing-session-id", // Optional
    command: "command-name", // Optional
  },
  toolContext,
)
```

### Task Result Structure

```typescript
interface TaskResult {
  title: string
  metadata: {
    sessionId: string
    summary: Array<{
      id: string
      tool: string
      state: {
        status: "running" | "completed" | "error"
        title?: string
      }
    }>
  }
  output: string
}
```

### Tool Context Structure

```typescript
interface ToolContext {
  agent: string
  messageID: string
  sessionID: string
  abort: AbortController
  async metadata(input: { title: string; metadata: any }): Promise<void>
  async ask(req: PermissionRequest): Promise<void>
}
```

## Permission System

### Check Permission

```typescript
await PermissionNext.ask({
  sessionID: sessionId,
  ruleset: agent.permission,
  permission: "tool-name",
  pattern: "*",
})
```

### Permission Ruleset Structure

```typescript
type PermissionAction = "allow" | "deny" | "ask"

interface PermissionRuleset {
  rules: Array<{
    permission: string
    pattern: string
    action: PermissionAction
  }>
}
```

### Default Permission Rules for Child Sessions

```typescript
const childPermissions = [
  { permission: "todowrite", pattern: "*", action: "deny" },
  { permission: "todoread", pattern: "*", action: "deny" },
  { permission: "task", pattern: "*", action: "deny" },
]
```

## Event Bus

### Subscribe to Events

```typescript
const unsubscribe = Bus.subscribe(MessageV2.Event.PartUpdated, (evt) => {
  const { part } = evt.properties
  if (part.sessionID === sessionId && part.type === "tool") {
    // Handle tool update
  }
})
```

### Unsubscribe

```typescript
unsubscribe()
```

### Available Events

```typescript
Session.Event.Created // { info: SessionInfo }
Session.Event.Updated // { info: SessionInfo }
MessageV2.Event.PartUpdated // { part: PartInfo }
Command.Event.Executed // { name: string; sessionID: string; messageID: string }
```

## Configuration

### Load Config

```typescript
const config = await Config.get()
```

### Config Structure

```typescript
interface Config {
  default_agent?: string
  model?: ModelConfig
  permission?: Record<string, PermissionConfig>
  agent?: Record<string, AgentConfig>
  experimental?: {
    primary_tools?: string[]
    openTelemetry?: boolean
  }
}
```

### Agent Config Structure

```typescript
interface AgentConfig {
  mode?: "primary" | "subagent" | "all"
  description?: string
  model?: ModelConfig
  temperature?: number
  top_p?: number
  color?: string
  prompt?: string
  permission?: Record<string, PermissionConfig>
  disable?: boolean
  steps?: number
  options?: Record<string, any>
}
```

## Message System

### Create Message

```typescript
const message = await Session.createMessage(sessionId, {
  role: "user" | "assistant" | "system",
  parts: [
    { type: "text", text: "Message content" },
    {
      type: "tool",
      tool: "tool-name",
      input: {
        /* tool params */
      },
    },
  ],
})
```

### Update Message

```typescript
await Session.updateMessage(messageId, {
  role: "assistant",
  finish: "tool-calls",
  time: { completed: Date.now() },
})
```

### Message Part Types

```typescript
type MessagePart =
  | { type: "text"; text: string }
  | { type: "tool"; tool: string; input: any; state: ToolState }
  | { type: "subtask"; agent: string; description: string; prompt: string }
  | { type: "image"; image: string }
```

### Tool State

```typescript
interface ToolState {
  status: "running" | "completed" | "error"
  input?: any
  output?: any
  metadata?: any
  title?: string
  error?: string
  time?: { start: number; end: number }
}
```

## Session Prompt

### Create Prompt

```typescript
const result = await SessionPrompt.prompt({
  sessionID: sessionId,
  messageID: messageId,
  model: { modelID, providerID },
  agent: agentName,
  parts: [
    { type: "text", text: "Prompt content" },
    // ... more parts
  ],
  tools: {
    toolName: true, // Enable tool
    toolName2: false, // Disable tool
  },
})
```

### Resolve Prompt Parts

```typescript
const parts = await SessionPrompt.resolvePromptParts(promptText)
```

### Prompt Result

```typescript
interface PromptResult {
  info: MessageInfo
  parts: MessagePart[]
}
```

## Command System

### Execute Command

```typescript
const result = await Command.execute({
  command: "command-name",
  arguments: ["arg1", "arg2"],
  sessionID: sessionId,
  messageID: messageId,
  model: { modelID, providerID },
  agent: "agent-name",
})
```

### Command Config

```typescript
interface CommandConfig {
  name: string
  agent: string
  description?: string
  subtask?: boolean
  command: string
}
```

## Built-in Agents

### Build Agent

```typescript
{
  name: "build",
  mode: "primary",
  description: "Default agent for development work",
  permission: { /* all tools allowed */ }
}
```

### Plan Agent

```typescript
{
  name: "plan",
  mode: "primary",
  description: "Read-only agent for analysis",
  permission: {
    edit: { "*": "deny" },
    bash: { "*": "ask" }
  }
}
```

### General Subagent

```typescript
{
  name: "general",
  mode: "subagent",
  description: "General-purpose agent for complex tasks",
  permission: {
    todowrite: { "*": "deny" },
    todoread: { "*": "deny" }
  }
}
```

### Explore Subagent

```typescript
{
  name: "explore",
  mode: "subagent",
  description: "Fast agent for codebase exploration",
  permission: {
    grep: { "*": "allow" },
    glob: { "*": "allow" },
    read: { "*": "allow" },
    bash: { "*": "allow" },
    webfetch: { "*": "allow" },
    websearch: { "*": "allow" },
    codesearch: { "*": "allow" }
  }
}
```

## Helper Functions

### Generate Agent

```typescript
const agentConfig = await Agent.generate({
  description: "Create a code reviewer agent",
  model: { modelID: "claude-sonnet-4", providerID: "anthropic" },
})

// Returns:
// {
//   identifier: "code-reviewer",
//   whenToUse: "After writing significant code",
//   systemPrompt: "You are a code reviewer..."
// }
```

### Merge Permissions

```typescript
const merged = PermissionNext.merge(defaultPermissions, userPermissions)
```

### Parse Model

```typescript
const model = Provider.parseModel({
  providerID: "anthropic",
  modelID: "claude-sonnet-4-20250514",
})
```

## Error Handling

### Common Errors

```typescript
// Agent not found
throw new Error(`Unknown agent type: ${agentName}`)

// Session not found
throw new NamedError.Unknown({ message: `Session not found: ${sessionId}` })

// Permission denied
throw new PermissionError.Denied({ permission, pattern })

// Invalid session data
throw new SessionError.InvalidData("Invalid session structure")
```

### Error Types

```typescript
// NamedError
NamedError.Unknown({ message: string })

// PermissionError
PermissionError.Denied({ permission, pattern })
PermissionError.Ask({ permission, pattern })

// SessionError
SessionError.NotFound(id)
SessionError.InvalidData(reason)
```

## Utility Functions

### Generate Identifier

```typescript
const messageID = Identifier.ascending("message")
const sessionID = Identifier.ascending("session")
```

### Bus Events

```typescript
// Publish event
Bus.publish(Session.Event.Updated, { info: session })

// Subscribe
const unsub = Bus.subscribe(EventType, handler)
```

### Async Utilities

```typescript
// Immediately invoked function expression
const result = await iife(async () => {
  if (condition) return value1
  return value2
})

// Deferred cleanup
using cleanup = defer(() => {
  abort.removeEventListener("abort", cancel)
})
```

## File Paths Reference

```
packages/opencode/src/
├── agent/
│   └── agent.ts              # Agent registry and built-in agents
├── tool/
│   ├── task.ts              # Task tool (lines 14-167)
│   └── task.txt             # Task description
├── session/
│   ├── index.ts             # Session management (create, get, update)
│   ├── prompt.ts            # Prompt processing (subtask logic at 351-376)
│   └── message-v2.ts        # Message and part definitions
├── permission/
│   └── next.ts              # Permission system
├── config/
│   └── config.ts            # Configuration loading (agent config at 467)
└── cli/cmd/
    └── agent.ts             # CLI agent commands
```

## Key Line Numbers

- Agent mode definition: `agent.ts:21`
- Task tool execute: `tool/task.ts:31-77`
- Subtask detection: `session/prompt.ts:1534`
- Permission checking: `session/prompt.ts:396-402`
- Session creation: `session/index.ts:39-79`
- Event subscription: `tool/task.ts:90-110`

## Common Patterns

### Pattern 1: Spawn Subagent

```typescript
const agent = await Agent.get("explore")
const session = await Session.create({
  parentID: currentSession.id,
  title: "Explore codebase",
  permission: [
    { permission: "todowrite", pattern: "*", action: "deny" },
    { permission: "todoread", pattern: "*", action: "deny" },
  ],
})

const result = await SessionPrompt.prompt({
  sessionID: session.id,
  agent: "explore",
  parts: [{ type: "text", text: "Find all API endpoints" }],
})
```

### Pattern 2: Track Progress

```typescript
const parts: Record<string, ToolState> = {}
const unsub = Bus.subscribe(MessageV2.Event.PartUpdated, (evt) => {
  if (evt.properties.part.sessionID !== session.id) return
  parts[evt.properties.part.id] = evt.properties.part.state

  ctx.metadata({
    title: task.description,
    metadata: { summary: Object.values(parts) },
  })
})
```

### Pattern 3: Navigate Hierarchy

```typescript
// Get next in hierarchy
let current = session
if (current.parentID) {
  const parent = await Session.get(current.parentID)
  const siblings = await Session.children(parent.id)
  const currentIndex = siblings.findIndex((s) => s.id === current.id)
  const next = siblings[(currentIndex + 1) % siblings.length]
}
```

## TypeScript Type Imports

```typescript
import { Agent } from "./agent/agent"
import { Session } from "./session"
import { Tool } from "./tool/tool"
import { PermissionNext } from "./permission/next"
import { Bus } from "./bus"
import { MessageV2 } from "./session/message-v2"
import { Config } from "./config/config"
import { Command } from "./command"
```

## Configuration File Locations

```
Global config:  ~/.config/opencode/opencode.json
Project config: .opencode/opencode.json
Agents (JSON):  ~/.config/opencode/agent/*.json
Agents (MD):     ~/.config/opencode/agent/*.md
```

## Environment Variables

```bash
OPENCODE_INSTALL_DIR  # Custom installation directory
XDG_BIN_DIR         # XDG compliant binary directory
OPENCODE_MODEL       # Default model
OPENCODE_PROVIDER    # Default provider
```

## Debugging

### Enable Logging

```typescript
const log = Log.create({ service: "task" })
log.info("Starting task execution")
log.error("Task failed", { error, agent: task.agent })
```

### Session State

```typescript
// Check if session is child
const isChild = !!session.parentID

// Check if default title
const isDefault = Session.isDefaultTitle(session.title)

// Get session age
const age = Date.now() - session.time.created
```

## Migration Checklist

For porting to Rust:

- [ ] Implement `AgentRegistry` with RwLock for thread safety
- [ ] Implement `SessionManager` with parent/child navigation
- [ ] Implement `TaskTool` with permission checking
- [ ] Implement `EventBus` using tokio broadcast channel
- [ ] Implement `PermissionChecker` with pattern matching
- [ ] Implement `MessageV2` part system
- [ ] Implement async/await with tokio runtime
- [ ] Add serde serialization for all structs
- [ ] Add thiserror for error handling
- [ ] Add unit tests for each component
