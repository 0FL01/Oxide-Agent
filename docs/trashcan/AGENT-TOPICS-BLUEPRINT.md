# Blueprint: Multi-Agent Architecture with Thread-Based Routing
## Executive Summary
This document describes OpenClaw's architecture for running multiple independent agents within a single platform (e.g., a Telegram supergroup), where each thread/topic can be served by a different agent. This pattern enables:
- **Topic/Thread-based isolation**: Each forum topic gets its own agent session
- **Dynamic agent binding**: Runtime binding of agents to specific conversations
- **Hierarchical routing**: Thread-level overrides with parent fallback
- **Session isolation**: Per-thread session keys prevent cross-contamination
**Target use case**: Porting this architecture to other platforms (Discord, Slack, Matrix, etc.)
---
## Architecture Overview
```
┌─────────────────────────────────────────────────────────────────────┐
│                         Platform (Telegram/Discord/etc.)             │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐         │
│  │  General  │  │ Topic #1  │  │ Topic #2  │  │ Topic #3  │         │
│  │ (Agent A) │  │ (Agent B) │  │ (Agent C) │  │ (Agent A) │         │
│  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘         │
└────────┼─────────────┼─────────────┼─────────────┼───────────────────┘
         │             │             │             │
         │ Incoming messages with thread_id/message_thread_id
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    Channel Layer (telegram/discord/...)            │
│  • Parse thread ID from incoming message                            │
│  • Resolve thread spec (scope: forum|dm|none)                        │
│  • Extract configuration for topic                                   │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      Routing Layer (src/routing/)                   │
│  • Apply topic-level agentId override (topicConfig.agentId)         │
│  • Check runtime thread bindings (dynamic)                          │
│  • Fallback to parent group binding if no thread match              │
│  • Build session key with thread suffix                              │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    Agent Delivery Layer                             │
│  • Route to appropriate agent based on resolved route               │
│  • Process with thread-isolated session                             │
│  • Store reply metadata (thread ID, reply-to-message ID)            │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    Outbound Layer                                   │
│  • Build thread-specific API params (message_thread_id/thread_id)    │
│  • Handle special cases (General topic, thread-not-found fallback)   │
│  • Send reply to correct thread/topic                                │
└─────────────────────────────────────────────────────────────────────┘
```
---
## Core Concepts
### 1. Thread Specification
A thread specification represents a conversation thread in the platform:
```typescript
// src/telegram/bot/helpers.ts:16-19
type ThreadSpec {
  id?: number;              // Thread ID from platform
  scope: "forum" | "dm" | "none";
}
```
**Scope semantics:**
- `"forum"`: Forum topic (Telegram forum threads, Discord threads in channels)
- `"dm"`: Direct message topics (platform-specific threading within DMs)
- `"none"`: Regular group messages without thread support
### 2. Thread-Based Session Keys
Session keys include thread suffix for isolation:
```
Format: agent:{agentId}:{channel}:{peerKind}:{peerId}:thread:{threadId}
Examples:
- Telegram forum topic: agent:assistant-1:telegram:group:-100123456:thread:42
- Discord thread: agent:agent-2:discord:thread:112233445566778899:thread:112233445566778999
- Telegram DM (no thread): agent:agent-3:telegram:direct:123456
```
**Key functions:**
- `src/routing/session-key.ts:234-252`: `resolveThreadSessionKeys()` adds thread suffix
- `src/telegram/bot/helpers.ts:174-176`: `buildTelegramGroupPeerId()` creates peer IDs
### 3. Agent Routing Tiers
Routing resolves agent ID in priority order:
```
1. Thread binding (runtime dynamic override)
   ↓ no match
2. Topic config override (static: topicConfig.agentId)
   ↓ no match
3. Parent group binding (thread inherits from group)
   ↓ no match
4. Group-level binding (binding.channel + binding.peer)
   ↓ no match
5. Account-level binding (binding.account)
   ↓ no match
6. Default agent
```
**Implementation:** `src/routing/resolve-route.ts:716-781`
### 4. Configuration Hierarchy
Configuration is merged from multiple levels:
```
Channel config
  ↓
Account config
  ↓
Group config (for forums)
  ↓
Topic config (highest priority for topics)
```
**Example (Telegram):**
```typescript
// src/config/types.telegram.ts:210-211
type TelegramGroupConfig {
  topics?: Record<string, TelegramTopicConfig>;  // topicId -> config
}
type TelegramTopicConfig {
  agentId?: string;           // Override agent for this topic
  enabled?: boolean;          // Disable bot in this topic
  requireMention?: boolean;   // Require @mention to trigger
  skills?: string[];          // Load only specific skills
  systemPrompt?: string;      // Override system prompt
  // ... other fields
}
```
---
## Data Structures
### Target Representation
```typescript
// src/telegram/targets.ts:1-5
type Target {
  chatId: string;             // Platform-specific chat ID
  messageThreadId?: number;   // Optional thread ID
  chatType: "direct" | "group" | "unknown";
}
```
### Thread Binding Record
For Discord (Telegram uses a simplified version):
```typescript
// src/discord/monitor/thread-bindings.types.ts:3-20
type ThreadBindingRecord {
  accountId: string;
  channelId: string;
  threadId: string;
  targetKind: "subagent" | "acp";
  targetSessionKey: string;
  agentId: string;
  webhookId?: string;
  webhookToken?: string;
  createdAt: number;
  lastMessageAt: number;
  expiresAt?: number;
}
```
### Session Entry
```typescript
// src/config/sessions/types.ts:68-167
type SessionEntry {
  sessionKey: string;
  agentId: string;
  channel: string;
  chatType: "direct" | "group";
  groupId?: string;
  threadId?: string;          // Platform-specific thread ID
  origin: {
    threadId?: string;        // Optional origin thread for context
  };
  lastRoute?: {
    agentId: string;
    bindingSessionKey?: string;
  };
  // ... metadata, timestamps
}
```
---
## Message Flow (Telegram Example)
### Inbound Flow
```
1. Platform message received
   └─> bot.on("message") in src/telegram/bot-handlers.ts:1491-1510
2. Extract thread information
   └─> message_thread_id, is_forum from message object
   └─> resolveTelegramThreadSpec() in src/telegram/bot/helpers.ts:100-122
3. Resolve configuration
   └─> resolveTelegramGroupConfig(chatId, threadId) for topic config
4. Route to agent
   └─> resolveTelegramConversationRoute() in src/telegram/conversation-route.ts:18-140
       • Apply topicConfig.agentId override (lines 54-88)
       • Check thread bindings (lines 102-133)
       • Build session key with thread suffix
5. Process message
   └─> Delivery with thread metadata preserved
```
### Outbound Flow
```
1. Build thread params
   └─> buildTelegramThreadParams() in src/telegram/bot/helpers.ts:138-154
       • Forum topics: return { message_thread_id: id } (except General topic)
       • General topic (id=1): return undefined (Telegram rejects message_thread_id=1)
       • DM topics: return { message_thread_id: id }
2. Send to platform
   └─> Include thread params in API call (sendMessage, sendPhoto, etc.)
   └─> Handle thread-not-found errors with fallback (DMs only)
3. Sequential processing
   └─> getTelegramSequentialKey() ensures order within thread
       Format: telegram:{chatId}:topic:{threadId}
```
---
## Implementation Guide for New Platforms
### Step 1: Parse Thread ID from Incoming Messages
Create a thread spec resolver function:
```typescript
function resolvePlatformThreadSpec(params: {
  isGroup: boolean;
  // Platform-specific flags
  hasThread: boolean;           // e.g., has_thread for Discord
  threadId?: string | null;     // Platform-specific thread ID
}): ThreadSpec {
  if (!isGroup) {
    // DM: check if platform supports DM threading
    return hasThread && threadId
      ? { id: parseThreadId(threadId), scope: "dm" }
      : { scope: "none" };
  }
  if (!hasThread) {
    // Regular group without threads
    return { scope: "none" };
  }
  // Forum/group with threads
  return { id: parseThreadId(threadId), scope: "forum" };
}
```
### Step 2: Build Platform-Specific Peer IDs
```typescript
function buildPlatformThreadPeerId(chatId: string, threadId?: number): string {
  return threadId != null
    ? `${chatId}:topic:${threadId}`    // Match Telegram format for consistency
    : chatId;
}
function buildPlatformParentPeer(params: {
  isGroup: boolean;
  threadId?: number;
  chatId: string;
}): { kind: "group" | "channel"; id: string } | undefined {
  if (!params.isGroup || params.threadId == null) {
    return undefined;
  }
  return { kind: "group", id: String(params.chatId) };
}
```
### Step 3: Route with Thread Override
```typescript
function resolvePlatformConversationRoute(params: {
  cfg: Config;
  accountId: string;
  chatId: string;
  isGroup: boolean;
  threadId?: number;
  senderId: string;
  // Platform-specific
  topicAgentId?: string;  // From per-thread config
}): Route {
  // Check topic override first (highest priority)
  if (topicAgentId != null) {
    return {
      agentId: topicAgentId,
      sessionKey: buildSessionKey(topicAgentId, chatId, threadId),
    };
  }
  // Check runtime thread bindings
  const binding = getSessionBindingService().resolveByConversation({
    conversationId: `${chatId}:topic:${threadId}`,
    accountId,
  });
  if (binding) {
    return {
      agentId: binding.agentId,
      sessionKey: binding.targetSessionKey,
    };
  }
  // Fallback to parent peer binding
  const parentPeer = buildPlatformParentPeer({ isGroup, threadId, chatId });
  const parentRoute = resolveRouteForPeer(parentPeer);
  if (parentRoute) {
    return parentRoute;
  }
  // Default fallback
  return resolveDefaultRoute(cfg, accountId, chatId);
}
```
### Step 4: Build Outbound Thread Params
```typescript
function buildPlatformThreadParams(thread?: ThreadSpec | null) {
  if (!thread || thread.scope === "none") {
    return undefined;
  }
  const id = thread.id;
  if (id == null) {
    return undefined;
  }
  // Platform-specific special cases
  if (thread.scope === "forum") {
    // Example: Telegram rejects message_thread_id=1 (General topic)
    if (id === 1) {
      return undefined;
    }
    return { message_thread_id: id };
  }
  if (thread.scope === "dm") {
    return { message_thread_id: id };
  }
  return undefined;
}
```
### Step 5: Handle Thread-Not-Found Errors
```typescript
async function sendWithThreadFallback<T>(params: {
  operation: string;
  thread: ThreadSpec | null;
  requestParams: Record<string, unknown>;
  platformSend: (params: Record<string, unknown>) => Promise<T>;
}): Promise<T> {
  const { thread, requestParams } = params;
  // DM threads may fail if topic doesn't exist
  const allowThreadlessRetry = thread?.scope === "dm";
  const hasThreadId = requestParams.message_thread_id != null;
  try {
    return await platformSend(requestParams);
  } catch (err) {
    if (!allowThreadlessRetry || !hasThreadId || !isThreadNotFoundError(err)) {
      throw err;
    }
    // Retry without thread ID
    const retryParams = { ...requestParams };
    delete retryParams.message_thread_id;
    return await platformSend(retryParams);
  }
}
```
### Step 6: Define Configuration Schema
```typescript
// Platform-specific per-thread config
type PlatformTopicConfig = {
  // Routing
  agentId?: string;
  // Behavior
  enabled?: boolean;
  requireMention?: boolean;
  groupPolicy?: "open" | "disabled" | "allowlist";
  // Skills
  skills?: string[];          // Limit skills for this thread
  // Access control
  allowFrom?: Array<string>;  // Platform-specific user IDs
  // Customization
  systemPrompt?: string;
};
```
---
## Platform-Specific Considerations
### Telegram (Forum Topics)
**Key characteristics:**
- `message_thread_id` from message object
- `is_forum` flag on chat object
- General topic ID = 1 (special case: cannot send with message_thread_id=1)
- Per-topic config in `TelegramGroupConfig.topics[topicId]`
**Gotchas:**
- General topic (id=1) must omit `message_thread_id` in API calls
- Thread-not-found fallback only for DM scope (not forum topics)
- Sequential processing keys: `telegram:{chatId}:topic:{threadId}`
### Discord (Threads)
**Key characteristics:**
- `thread_id` for threads in forums/channels
- `thread.parent_id` for parent channel
- Thread bindings with webhook support for real-time updates
- Thread lifecycle: created when subagent spawns
**Additional features:**
- Thread bindings with expiration (idle timeout, max age)
- Webhook-based delivery for spawned threads
- Thread ownership tracking (who created the thread)
### Slack (Huddles/Threads)
**Key characteristics:**
- Thread-based conversations within channels
- `thread_ts` timestamp for thread identification
- Reply-to-message threading model
**Considerations:**
- Slack API uses `thread_ts` for threading
- Parent message required for thread creation
- Different permission model than Discord/Telegram
---
## Edge Cases & Gotchas
### 1. General Topic Special Case (Telegram)
**Problem:** Telegram rejects `message_thread_id=1` in API calls, but it's the valid ID for the General forum topic.
**Solution:**
```typescript
function buildTelegramThreadParams(thread?: ThreadSpec | null) {
  if (!thread || thread.scope !== "forum") {
    return undefined;
  }
  const id = thread.id;
  if (id == null) return undefined;
  // General topic (id=1) -> omit message_thread_id
  if (id === 1) return undefined;
  return { message_thread_id: id };
}
```
### 2. Thread-Not-Found Fallback Ambiguity
**Problem:** If a topic is deleted while processing a message, thread-not-found fallback may send the reply to the wrong location.
**Mitigation:**
- Only apply fallback for `scope === "dm"` (not forum topics)
- Log the fallback for debugging
- Consider user notification in production
### 3. Session Key Collision Risk
**Problem:** Changing `dmScope` configuration after sessions are created could orphan existing data.
**Mitigation:**
- Document `dmScope` migration strategy
- Consider session key versioning
- Provide migration tooling
### 4. Thread Binding Inheritance Complexity
**Problem:** Multi-agent scenarios in single groups rely on parent peer fallback, which may create ambiguous routing if both parent and child have conflicting bindings.
**Mitigation:**
- Clear priority: thread > parent > group > account > default
- Document override rules
- Visualize binding hierarchy in config tooling
### 5. Cross-Account Thread Binding Behavior
**Problem:** Unclear behavior when a thread binding references a targetSessionKey with a different accountId than the current platform account.
**Recommendation:**
- Validate accountId consistency when resolving bindings
- Add explicit error or log warning for cross-account bindings
- Document expected behavior
---
## Configuration Patterns
### Pattern 1: Static Topic-to-Agent Mapping
Use per-topic `agentId` in config:
```typescript
// config.yaml
channels:
  telegram:
    accounts:
      - accountId: "my-bot"
        groups:
          - chatId: "-1001234567890"
            topics:
              "1":                     # General topic
                agentId: "assistant-general"
                requireMention: false
              "42":                    # Support tickets
                agentId: "assistant-support"
                requireMention: true
              "99":                    # Development
                agentId: "assistant-dev"
                skills: ["code", "github"]
```
### Pattern 2: Dynamic Thread Bindings
Use runtime thread bindings (spawned by subagents):
```typescript
// src/discord/monitor/thread-bindings.lifecycle.ts:120-196
async function autoBindSpawnedPlatformSubagent(params: {
  parentThreadBinding?: ThreadBindingRecord;
  agentId: string;
  threadId: string;
  channelId: string;
  accountId: string;
}): Promise<ThreadBindingRecord> {
  const binding: ThreadBindingRecord = {
    accountId,
    channelId,
    threadId,
    targetKind: "subagent",
    targetSessionKey: `${params.threadId}`,
    agentId: params.agentId,
    createdAt: Date.now(),
    lastMessageAt: Date.now(),
  };
  await getSessionBindingService().saveBinding(binding);
  return binding;
}
```
### Pattern 3: Parent Group Fallback
Configure group-level binding with thread-specific overrides:
```typescript
// Default agent for entire group
bindings:
  - type: route
    channel: telegram
    accountId: my-bot
    peer:
      kind: group
      id: -1001234567890
    agentId: assistant-general
// Override specific topics
channels:
  telegram:
    accounts:
      - accountId: my-bot
        groups:
          - chatId: "-1001234567890"
            topics:
              "42":
                agentId: assistant-support  # Overrides group binding
```
---
## Testing Strategy
### Unit Tests
1. **Thread spec resolution:**
   - Test forum thread parsing
   - Test DM thread parsing
   - Test no-thread scenarios
2. **Routing logic:**
   - Test topic override priority
   - Test thread binding override
   - Test parent fallback behavior
3. **Thread param building:**
   - Test General topic omission
   - Test forum thread inclusion
   - Test DM thread inclusion
### Integration Tests
1. **Full message flow:**
   - Send message to forum topic
   - Verify route to correct agent
   - Verify reply goes to same topic
2. **Thread binding lifecycle:**
   - Create thread binding
   - Verify routing override
   - Verify session isolation
3. **Error handling:**
   - Test thread-not-found fallback
   - Test disabled topic rejection
   - Test missing config fallback
---
## Migration Checklist for New Platform
- [ ] Define platform-specific thread ID format
- [ ] Create `resolvePlatformThreadSpec()` function
- [ ] Implement `buildPlatformThreadPeerId()` function
- [ ] Implement `buildPlatformParentPeer()` function
- [ ] Add thread spec extraction to message handler
- [ ] Add thread-specific config resolution
- [ ] Integrate with `resolveAgentRoute()` (routing layer)
- [ ] Implement `buildPlatformThreadParams()` for outbound
- [ ] Add thread-not-found error handling
- [ ] Define platform-specific `TopicConfig` type
- [ ] Add thread suffix to session key generation
- [ ] Update channel tests for thread scenarios
- [ ] Document platform-specific gotchas
- [ ] Add examples to documentation
---
## References
### Core Implementation Files
- **Routing:** `src/routing/resolve-route.ts:614-804` (agent route resolution)
- **Session Keys:** `src/routing/session-key.ts:234-252` (thread suffix)
- **Thread Bindings:** `src/discord/monitor/thread-bindings.lifecycle.ts:120-196`
- **Bindings Policy:** `src/channels/thread-bindings-policy.ts:109-138`
### Telegram-Specific
- **Topic Detection:** `src/telegram/bot-message-context.ts:65-76`
- **Thread Spec:** `src/telegram/bot/helpers.ts:100-122` (`resolveTelegramThreadSpec`)
- **Route Resolution:** `src/telegram/conversation-route.ts:18-140`
- **Outbound Params:** `src/telegram/bot/helpers.ts:138-154` (`buildTelegramThreadParams`)
- **Config Types:** `src/config/types.telegram.ts:183-199` (`TelegramTopicConfig`)
- **Target Parsing:** `src/telegram/targets.ts:72-116` (`parseTelegramTarget`)
### Discord-Specific
- **Thread Bindings:** `src/discord/monitor/thread-bindings.types.ts:3-20`
- **Message Handler:** `src/discord/monitor/message-handler.preflight.ts:353-393`
- **Route Resolution:** `src/discord/monitor/route-resolution.ts:80-100`
### Configuration
- **Bindings:** `src/config/bindings.ts:16-26` (binding matching)
- **Session Storage:** `src/config/sessions/types.ts:68-167` (`SessionEntry`)
- **Agent Types:** `src/config/types.agents.ts:28-44` (`AgentBindingMatch`)
---
## Appendix: Full Thread Spec Resolution (Telegram)
```typescript
// src/telegram/bot/helpers.ts:100-122
export function resolveTelegramThreadSpec(params: {
  isGroup: boolean;
  isForum?: boolean;
  messageThreadId?: number | null;
}): TelegramThreadSpec {
  const { isGroup, isForum, messageThreadId } = params;
  if (!isGroup) {
    // DM: message_thread_id represents DM topics (if platform supports)
    if (messageThreadId != null) {
      return { id: messageThreadId, scope: "dm" };
    }
    return { scope: "none" };
  }
  if (!isForum) {
    // Regular group: reply threads are not treated as topics
    return { scope: "none" };
  }
  // Forum group: message_thread_id is the topic ID
  if (messageThreadId != null) {
    return { id: messageThreadId, scope: "forum" };
  }
  // Forum group without explicit thread ID -> default to General topic
  return { id: 1, scope: "forum" };
}
```
---
## Summary
This architecture enables sophisticated multi-agent deployments within a single platform instance:
1. **Thread-based isolation** through session key suffixes
2. **Hierarchical routing** with clear priority rules
3. **Static and dynamic binding** support
4. **Platform-specific special case handling**
The core concepts (thread specs, peer IDs, route tiers) are portable across platforms, with platform-specific implementations for:
- Thread ID parsing
- API parameter building
- Configuration schema
- Special case handling (General topic, etc.)
When porting to a new platform, follow the **Migration Checklist** and adapt the **Platform-Specific Considerations** section to your target platform's API and threading model.