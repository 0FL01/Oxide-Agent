# LLM Coder Quick Reference - Sub-Agents

## What Are Sub-Agents?

Sub-agents are specialized AI assistants that can be invoked by the primary agent to handle specific tasks. They run in isolated child sessions with restricted permissions.

**Key Concept**: Parent agent → spawns child session → subagent executes task → returns result to parent → user sees summary

## When to Use Sub-Agents

### ✅ DO Use For:

- Multi-step research tasks (e.g., "Analyze authentication flow across 10 files")
- Codebase exploration (e.g., "Find all API endpoints and document them")
- Parallel work (e.g., "Check tests in three different directories simultaneously")
- Complex searches (e.g., "Find all database queries and check for SQL injection")
- Task delegation (e.g., "Let the explore agent find files, then general agent analyze them")

### ❌ DON'T Use For:

- Single file reads (`Read` tool is faster)
- Simple string replacements (`Edit` tool is direct)
- Basic file searches (`Glob` tool is better)
- Direct user communication (primary agent should handle this)
- One-liner commands or quick lookups

## Built-in Sub-Agents

### Explore Agent

**Use when**: Fast codebase exploration needed
**Tools**: grep, glob, read, bash, webfetch, websearch, codesearch
**Thoroughness**: "quick" | "medium" | "very thorough"
**Example**: "@explore find all API endpoints in src/api directory"

### General Agent

**Use when**: Complex multi-step tasks, parallel execution
**Tools**: Most tools except todowrite/todoread
**Example**: "@general refactor this code for better performance"

## How to Invoke Sub-Agents

### Method 1: @ Mention (User-Initiated)

```
User: @explore help me understand the authentication system
Primary Agent: [Detects @explore] → spawns explore subagent → presents results
```

### Method 2: Automatic (Primary Agent Decision)

```
User: Help me refactor the database layer
Primary Agent: [Detects complex multi-step task] → calls @general subagent automatically
```

### Method 3: Commands (Pre-configured)

```
User: /analyze-security path/to/code
Primary Agent: [Command configured with subtask: true] → spawns security-analyzer subagent
```

## Sub-Agent Execution Flow

```
1. Primary agent decides to use subagent
2. ↓
3. Calls Task tool with:
   - description: "Short task name"
   - prompt: "Detailed instructions"
   - subagent_type: "agent-name"
4. ↓
5. Task tool checks permissions
6. ↓
7. Creates child session with restricted permissions
8. ↓
9. Subagent executes in child session
10. ↓
11. Task tool subscribes to progress events
12. ↓
13. Subagent completes task
14. ↓
15. Task tool aggregates tool results
16. ↓
17. Returns summary to primary agent
18. ↓
19. Primary agent presents results to user
```

## Child Session Permissions

Child sessions have restricted access to prevent issues:

```typescript
// Always denied in child sessions:
- todowrite  // Prevent recursive task creation
- todoread   // Prevent TODO list access
- task        // Prevent subagent spawning subagents

// Can be optionally allowed:
- read, write, edit, bash, etc. (depends on config)
```

## Progress Tracking

Sub-agent progress is tracked via event bus:

```typescript
// Parent subscribes to child's tool updates
Bus.subscribe(MessageV2.Event.PartUpdated, (evt) => {
  if (evt.part.sessionID === childSessionID) {
    // Update parent metadata with child's progress
    ctx.metadata({
      title: "Task in progress",
      metadata: {
        summary: childToolResults,
        sessionId: childSessionID,
      },
    })
  }
})
```

## Session Navigation

Users can inspect sub-agent sessions:

```
<Leader>+Right  : parent → child1 → child2 → ... → parent
<Leader>+Left   : parent ← child1 ← child2 ← ... → parent
```

This allows users to see exactly what subagents did.

## Common Patterns

### Pattern 1: Explore Then General

```
1. Call @explore to find relevant files
2. Receive file list
3. Call @general to analyze those files
4. Present combined results
```

### Pattern 2: Parallel Exploration

```
1. Call @explore (thoroughness: "quick") for directory A
2. Call @explore (thoroughness: "medium") for directory B
3. Call @explore (thoroughness: "very thorough") for directory C
4. Wait for all to complete
5. Synthesize results
```

### Pattern 3: Chain Subagents

```
Parent (build)
├── Child 1 (@explore) finds files
└── Child 2 (@general) analyzes findings
```

## Creating Custom Sub-Agents

### JSON Configuration

Create `~/.config/opencode/agent/code-reviewer.json`:

```json
{
  "mode": "subagent",
  "description": "Reviews code for best practices and potential issues",
  "model": "anthropic/claude-sonnet-4-20250514",
  "permission": {
    "write": "deny",
    "edit": "deny",
    "bash": "deny"
  },
  "prompt": "You are a code reviewer. Focus on security, performance, and maintainability."
}
```

### Markdown Configuration

Create `~/.config/opencode/agent/code-reviewer.md`:

```markdown
---
description: Reviews code for best practices
mode: subagent
model: anthropic/claude-sonnet-4-20250514
temperature: 0.1
---

You are a code reviewer. Focus on:

- Code quality and best practices
- Potential bugs and edge cases
- Performance implications
- Security considerations

Provide constructive feedback without making direct changes.
```

## Task Tool Parameters

```typescript
{
  description: "Short (3-5 words)",      // Shown in UI
  prompt: "Detailed task instructions",     // Actual prompt
  subagent_type: "agent-name",            // Which subagent to use
  session_id: "existing-session-id",      // Optional: continue existing
  command: "command-name"                  // Optional: triggered by command
}
```

## Result Structure

```typescript
{
  title: "Task description",
  metadata: {
    sessionId: "child-session-id",
    summary: [
      { id: "tool-1", tool: "grep", state: { status: "completed", title: "Found 5 results" } },
      { id: "tool-2", tool: "read", state: { status: "completed", title: "Read 3 files" } }
    ]
  },
  output: "Text output from subagent\n\n<task_metadata>\nsession_id: child-session-id\n</task_metadata>"
}
```

## Best Practices

### For Primary Agents

1. **Be Specific**: Give clear, detailed prompts to subagents
2. **Set Expectations**: Tell subagent what to return (e.g., "Return a JSON summary")
3. **Summarize**: Subagent results aren't visible to users - provide context
4. **Use Parallelism**: Launch multiple subagents when possible
5. **Handle Errors**: Catch and report subagent failures gracefully

### For Sub-Agent Developers

1. **Clear Descriptions**: Describe when to use the subagent
2. **Restrictive Permissions**: Default to deny dangerous operations
3. **Focused Scope**: Single, well-defined purpose per subagent
4. **Appropriate Models**: Use right model for task (faster for simple, smarter for complex)

### For Users

1. **@ Mention Control**: Explicitly invoke subagents when needed
2. **Inspect Sessions**: Use navigation to see what subagents did
3. **Customize**: Create project-specific subagents for common workflows

## Debugging Sub-Agents

### Check Sub-Agent Execution

```typescript
// 1. Verify subagent exists
const agent = await Agent.get(subagent_type)
if (!agent) throw new Error(`Unknown agent: ${subagent_type}`)

// 2. Check permissions
await PermissionNext.ask({
  permission: "task",
  patterns: [subagent_type],
})

// 3. Monitor progress
const parts = {}
Bus.subscribe(MessageV2.Event.PartUpdated, (evt) => {
  if (evt.part.sessionID === childSessionID) {
    console.log("Tool:", evt.part.tool, "Status:", evt.part.state.status)
  }
})
```

### Common Issues

**Issue**: Subagent not found
**Fix**: Check agent is defined in config and `mode` is not `"primary"`

**Issue**: Permission denied
**Fix**: Check primary agent has `"task": "allow"` permission

**Issue**: Child session not created
**Fix**: Check session manager is initialized and has create permissions

**Issue**: No progress updates
**Fix**: Verify event subscription is set up correctly and session IDs match

## Rust Implementation Checklist

When porting to Rust, ensure:

- [ ] Agent registry with `Arc<RwLock<HashMap>>`
- [ ] Session manager with parent/child relationships
- [ ] Task tool with permission checking
- [ ] Event bus using `tokio::sync::broadcast`
- [ ] Permission system with pattern matching
- [ ] Message and part system with serialization
- [ ] Async/await with tokio runtime
- [ ] Error handling with `thiserror`
- [ ] Thread-safe shared state
- [ ] UUID generation for sessions/messages

## Key File Locations

```
Task tool:         packages/opencode/src/tool/task.ts:14-167
Agent registry:    packages/opencode/src/agent/agent.ts:16-253
Subtask detection:  packages/opencode/src/session/prompt.ts:1534
Permission check:   packages/opencode/src/session/prompt.ts:396-402
Session create:     packages/opencode/src/session/index.ts:39-79
Event subscribe:    packages/opencode/src/tool/task.ts:90-110
```

## Quick Examples

### Explore Codebase

```typescript
const result = await TaskTool.execute(
  {
    description: "Find API endpoints",
    prompt: "Find all API endpoints in src/api directory and document their functionality",
    subagent_type: "explore",
  },
  ctx,
)
```

### Parallel Tasks

```typescript
const [result1, result2] = await Promise.all([
  TaskTool.execute(
    {
      description: "Find tests",
      prompt: "Find all test files in src/",
      subagent_type: "explore",
    },
    ctx,
  ),
  TaskTool.execute(
    {
      description: "Analyze coverage",
      prompt: "Analyze test coverage gaps",
      subagent_type: "general",
    },
    ctx,
  ),
])
```

### Chain Subagents

```typescript
// 1. Find files
const exploreResult = await TaskTool.execute(
  {
    description: "Find relevant files",
    prompt: "Find all files related to user authentication",
    subagent_type: "explore",
  },
  ctx,
)

// 2. Analyze findings
const analysisResult = await TaskTool.execute(
  {
    description: "Analyze auth files",
    prompt: `Analyze these authentication files:\n${exploreResult.output}`,
    subagent_type: "general",
  },
  ctx,
)
```

## Performance Tips

1. **Cache Results**: Reuse session_id for repeated tasks
2. **Limit Scope**: Be specific about what subagent should search
3. **Parallelize**: Launch multiple subagents when tasks are independent
4. **Use Explore First**: Let @explore narrow down files before @general analyzes
5. **Set Timeouts**: Prevent subagents from running indefinitely

## Security Considerations

1. **Permission Isolation**: Child sessions inherit restrictive permissions
2. **Recursion Prevention**: `task` tool denied in child sessions
3. **User Consent**: Permission checks before spawning subagents
4. **Session Visibility**: Users can inspect all subagent sessions
5. **Configuration Control**: Users control which subagents are available

## Testing Sub-Agents

### Unit Tests

```typescript
describe("Task Tool", () => {
  it("should create child session", async () => {
    const result = await TaskTool.execute(params, ctx)
    expect(result.metadata.sessionId).toBeDefined()
  })

  it("should deny recursion", async () => {
    // Child session should not have task tool
    const childSession = await Session.get(result.metadata.sessionId)
    expect(childSession.permission).toContainEqual({
      permission: "task",
      pattern: "*",
      action: "deny",
    })
  })
})
```

### Integration Tests

```typescript
describe("Subagent Workflow", () => {
  it("should explore then analyze", async () => {
    // Start primary session
    const primary = await Session.create(...)

    // Invoke explore subagent
    const exploreResult = await TaskTool.execute({
      description: "Explore codebase",
      prompt: "Find all TypeScript files",
      subagent_type: "explore"
    }, createContext(primary))

    expect(exploreResult.output).toContain("Found")
  })
})
```

## Glossary

- **Primary Agent**: Main assistant users interact with directly
- **Sub-Agent**: Specialized assistant invoked by primary agent
- **Child Session**: Isolated session where subagent executes
- **Parent Session**: Original session that spawned subagent
- **Task Tool**: Mechanism for spawning and managing subagents
- **Permission Ruleset**: Set of rules defining what operations are allowed
- **Event Bus**: Pub/sub system for tracking progress
- **Session Hierarchy**: Tree structure of parent-child sessions

## Further Reading

- `subagent_architecture.md` - Full architectural documentation
- `rust_examples.md` - Rust implementation examples
- `api_reference.md` - Complete API reference
- OpenCode Docs: https://opencode.ai/docs/agents

## Support

- Documentation: https://opencode.ai/docs
- Discord: https://opencode.ai/discord
- GitHub Issues: https://github.com/anomalyco/opencode/issues
