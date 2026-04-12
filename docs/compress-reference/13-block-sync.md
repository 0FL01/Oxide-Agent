# 13. Block Sync (deactivation/reactivation)

> Source: `lib/messages/sync.ts`

---

```typescript
// lib/messages/sync.ts
export const syncCompressionBlocks = (
    state: SessionState,
    logger: Logger,
    messages: WithParts[],
): void => {
    const messagesState = state.prune.messages
    if (!messagesState?.blocksById?.size) return

    const messageIds = new Set(messages.map((msg) => msg.info.id))

    // Snapshot previous active blocks
    const previousActiveBlockIds = new Set<number>(
        Array.from(messagesState.blocksById.values())
            .filter((block) => block.active)
            .map((block) => block.blockId),
    )

    // Reset active state — will be recalculated
    messagesState.activeBlockIds.clear()
    messagesState.activeByAnchorMessageId.clear()

    const orderedBlocks = Array.from(messagesState.blocksById.values()).sort(
        (a, b) => a.createdAt - b.createdAt || a.blockId - b.blockId,
    )

    for (const block of orderedBlocks) {
        // DEACTIVATE: block's origin (compress tool call) message no longer exists
        const hasOriginMessage =
            typeof block.compressMessageId === "string" &&
            block.compressMessageId.length > 0 &&
            messageIds.has(block.compressMessageId)

        if (!hasOriginMessage) {
            block.active = false
            block.deactivatedAt = Date.now()
            continue
        }

        // DEACTIVATE: user manually deactivated
        if (block.deactivatedByUser) {
            block.active = false
            continue
        }

        // DEACTIVATE consumed blocks (they are nested inside this one)
        for (const consumedBlockId of block.consumedBlockIds) {
            if (!messagesState.activeBlockIds.has(consumedBlockId)) continue
            const consumedBlock = messagesState.blocksById.get(consumedBlockId)
            if (consumedBlock) {
                consumedBlock.active = false
                consumedBlock.deactivatedAt = Date.now()
                consumedBlock.deactivatedByBlockId = block.blockId
                messagesState.activeByAnchorMessageId.delete(consumedBlock.anchorMessageId)
            }
            messagesState.activeBlockIds.delete(consumedBlockId)
        }

        // REACTIVATE this block
        block.active = true
        block.deactivatedAt = undefined
        block.deactivatedByBlockId = undefined
        messagesState.activeBlockIds.add(block.blockId)
        if (messageIds.has(block.anchorMessageId))
            messagesState.activeByAnchorMessageId.set(block.anchorMessageId, block.blockId)
    }

    // Rebuild activeBlockIds for each message entry
    for (const entry of messagesState.byMessageId.values()) {
        entry.activeBlockIds = entry.allBlockIds.filter((id) =>
            messagesState.activeBlockIds.has(id),
        )
    }
}
```
