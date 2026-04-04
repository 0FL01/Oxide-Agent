# 6. Compression State Application

> Source: `lib/compress/state.ts`

---

## 6.1 Summary wrapping (stored format)

```typescript
// lib/compress/state.ts
export const COMPRESSED_BLOCK_HEADER = "[Compressed conversation section]"
const COMPRESSED_BLOCK_FOOTER = "[/Compressed conversation section]"

export function wrapCompressedSummary(blockId: number, summary: string): string {
    const header = `${COMPRESSED_BLOCK_HEADER} b${blockId}`
    const footer = COMPRESSED_BLOCK_FOOTER

    const body = summary.trim()
    if (body.length === 0) return `${header}\n${footer}`
    return `${header}\n${body}\n\n${footer}`
}
```

## 6.2 Full state application

```typescript
// lib/compress/state.ts
export function applyCompressionState(
    state: SessionState,
    input: CompressionStateInput,
    selection: SelectionResolution,
    anchorMessageId: string,
    blockId: number,
    summary: string,
    consumedBlockIds: number[],
): AppliedCompressionResult {
    const messagesState = state.prune.messages
    const consumed = [...new Set(consumedBlockIds.filter((id) => Number.isInteger(id) && id > 0))]
    const included = [...consumed]

    // Merge effective IDs from consumed blocks
    const effectiveMessageIds = new Set<string>(selection.messageIds)
    const effectiveToolIds = new Set<string>(selection.toolIds)
    for (const consumedBlockId of consumed) {
        const consumedBlock = messagesState.blocksById.get(consumedBlockId)
        if (!consumedBlock) continue
        for (const id of consumedBlock.effectiveMessageIds) effectiveMessageIds.add(id)
        for (const id of consumedBlock.effectiveToolIds) effectiveToolIds.add(id)
    }

    // Snapshot currently active messages/tools (for delta calculation)
    const initiallyActiveMessages = new Set<string>()
    for (const messageId of effectiveMessageIds) {
        const entry = messagesState.byMessageId.get(messageId)
        if (entry && entry.activeBlockIds.length > 0) initiallyActiveMessages.add(messageId)
    }

    // Create block
    const block: CompressionBlock = {
        blockId,
        runId: input.runId,
        active: true,
        deactivatedByUser: false,
        compressedTokens: 0,
        summaryTokens: input.summaryTokens,
        mode: input.mode,
        topic: input.topic,
        batchTopic: input.batchTopic,
        startId: input.startId,
        endId: input.endId,
        anchorMessageId,
        compressMessageId: input.compressMessageId,
        includedBlockIds: included,
        consumedBlockIds: consumed,
        parentBlockIds: [],
        directMessageIds: [],
        directToolIds: [],
        effectiveMessageIds: [...effectiveMessageIds],
        effectiveToolIds: [...effectiveToolIds],
        createdAt: Date.now(),
        summary,
    }

    // Register block
    messagesState.blocksById.set(blockId, block)
    messagesState.activeBlockIds.add(blockId)
    messagesState.activeByAnchorMessageId.set(anchorMessageId, blockId)

    // Deactivate consumed blocks
    for (const consumedBlockId of consumed) {
        const consumedBlock = messagesState.blocksById.get(consumedBlockId)
        if (!consumedBlock || !consumedBlock.active) continue

        consumedBlock.active = false
        consumedBlock.deactivatedAt = Date.now()
        consumedBlock.deactivatedByBlockId = blockId
        consumedBlock.parentBlockIds.push(blockId)

        messagesState.activeBlockIds.delete(consumedBlockId)
        const mapped = messagesState.activeByAnchorMessageId.get(consumedBlock.anchorMessageId)
        if (mapped === consumedBlockId)
            messagesState.activeByAnchorMessageId.delete(consumedBlock.anchorMessageId)
    }

    // Remove old block IDs from message entries
    for (const consumedBlockId of consumed) {
        const consumedBlock = messagesState.blocksById.get(consumedBlockId)
        if (!consumedBlock) continue
        for (const messageId of consumedBlock.effectiveMessageIds) {
            const entry = messagesState.byMessageId.get(messageId)
            if (!entry) continue
            entry.activeBlockIds = entry.activeBlockIds.filter((id) => id !== consumedBlockId)
        }
    }

    // Add new block ID to message entries
    for (const messageId of selection.messageIds) {
        const tokenCount = selection.messageTokenById.get(messageId) || 0
        const existing = messagesState.byMessageId.get(messageId)

        if (!existing) {
            messagesState.byMessageId.set(messageId, {
                tokenCount,
                allBlockIds: [blockId],
                activeBlockIds: [blockId],
            })
            continue
        }

        existing.tokenCount = Math.max(existing.tokenCount, tokenCount)
        if (!existing.allBlockIds.includes(blockId)) existing.allBlockIds.push(blockId)
        if (!existing.activeBlockIds.includes(blockId)) existing.activeBlockIds.push(blockId)
    }

    // Calculate newly compressed tokens (delta)
    let compressedTokens = 0
    const newlyCompressedMessageIds: string[] = []
    for (const messageId of effectiveMessageIds) {
        const entry = messagesState.byMessageId.get(messageId)
        if (!entry) continue

        const isNowActive = entry.activeBlockIds.length > 0
        const wasActive = initiallyActiveMessages.has(messageId)
        if (isNowActive && !wasActive) {
            compressedTokens += entry.tokenCount
            newlyCompressedMessageIds.push(messageId)
        }
    }

    block.directMessageIds = [...newlyCompressedMessageIds]
    block.compressedTokens = compressedTokens

    state.stats.pruneTokenCounter += compressedTokens
    state.stats.totalPruneTokens += state.stats.pruneTokenCounter
    state.stats.pruneTokenCounter = 0

    return {
        compressedTokens,
        messageIds: selection.messageIds,
        newlyCompressedMessageIds,
        newlyCompressedToolIds: [],
    }
}
```
