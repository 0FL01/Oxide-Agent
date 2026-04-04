# 1. Data Structures

> Source: `lib/state/types.ts`

---

## 1.1 Core message wrapper

```typescript
export interface WithParts {
    info: Message // { id, sessionID, role, agent, model, time, variant }
    parts: Part[] // [{ type: "text", text } | { type: "tool", callID, tool, state }]
}
```

## 1.2 Compression block

```typescript
export type CompressionMode = "range" | "message"

export interface CompressionBlock {
    blockId: number
    runId: number // groups blocks from same tool call
    active: boolean
    deactivatedByUser: boolean
    compressedTokens: number // tokens saved by this compression
    summaryTokens: number // tokens in the summary itself
    mode?: CompressionMode
    topic: string
    batchTopic?: string
    startId: string // boundary ID (e.g. "m0001" or "b2")
    endId: string
    anchorMessageId: string // where summary is injected in the message stream
    compressMessageId: string // the tool-call message that created this block
    includedBlockIds: number[] // all consumed + referenced block IDs
    consumedBlockIds: number[] // blocks whose summaries were folded into this one
    parentBlockIds: number[] // blocks that later consumed this one (reverse link)
    directMessageIds: string[] // messages newly compressed (not inherited)
    directToolIds: string[]
    effectiveMessageIds: string[] // all messages covered (direct + inherited)
    effectiveToolIds: string[]
    createdAt: number
    deactivatedAt?: number
    deactivatedByBlockId?: number
    summary: string // full wrapped summary text
}
```

## 1.3 Prune state

```typescript
export interface PrunedMessageEntry {
    tokenCount: number
    allBlockIds: number[] // every block this message has ever been part of
    activeBlockIds: number[] // currently active blocks covering this message
}

export interface PruneMessagesState {
    byMessageId: Map<string, PrunedMessageEntry>
    blocksById: Map<number, CompressionBlock>
    activeBlockIds: Set<number>
    activeByAnchorMessageId: Map<string, number> // anchor msg ID -> block ID
    nextBlockId: number
    nextRunId: number
}

export interface Prune {
    tools: Map<string, number> // tool call ID -> token count (strategy-pruned)
    messages: PruneMessagesState // compression state
}
```

## 1.4 Full session state

```typescript
export interface SessionState {
    sessionId: string | null
    isSubAgent: boolean
    manualMode: false | "active" | "compress-pending"
    compressPermission: "ask" | "allow" | "deny" | undefined
    pendingManualTrigger: PendingManualTrigger | null
    prune: Prune
    nudges: Nudges
    stats: SessionStats
    toolParameters: Map<string, ToolParameterEntry> // tool call ID -> metadata
    subAgentResultCache: Map<string, string>
    toolIdList: string[]
    messageIds: MessageIdState // raw ID <-> stable ref (m0001) bidirectional map
    lastCompaction: number
    currentTurn: number
    variant: string | undefined
    modelContextLimit: number | undefined
    systemPromptTokens: number | undefined
}
```

## 1.5 Tool parameter cache

```typescript
export interface ToolParameterEntry {
    tool: string
    parameters: any
    status?: ToolStatus // "pending" | "running" | "completed" | "error"
    error?: string
    turn: number // which conversation turn
    tokenCount?: number
}
```
