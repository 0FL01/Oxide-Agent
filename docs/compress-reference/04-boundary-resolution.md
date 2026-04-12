# 4. Boundary Resolution & Range Selection

> Source: `lib/compress/search.ts`, `lib/compress/range-utils.ts`

---

## 4.1 Boundary lookup construction

```typescript
// lib/compress/search.ts
function buildBoundaryLookup(
    context: SearchContext,
    state: SessionState,
): Map<string, BoundaryReference> {
    const lookup = new Map<string, BoundaryReference>()

    // Map message refs (m0001, m0002, ...) to raw messages
    for (const [messageRef, messageId] of state.messageIds.byRef) {
        const rawMessage = context.rawMessagesById.get(messageId)
        if (!rawMessage) continue
        if (isIgnoredUserMessage(rawMessage)) continue

        const rawIndex = context.rawIndexById.get(messageId)
        if (rawIndex === undefined) continue

        lookup.set(messageRef, {
            kind: "message",
            rawIndex,
            messageId,
        })
    }

    // Map block refs (b1, b2, ...) to their anchor messages
    const summaries = Array.from(context.summaryByBlockId.values()).sort(
        (a, b) => a.blockId - b.blockId,
    )

    for (const summary of summaries) {
        const anchorMessage = context.rawMessagesById.get(summary.anchorMessageId)
        if (!anchorMessage) continue
        if (isIgnoredUserMessage(anchorMessage)) continue

        const rawIndex = context.rawIndexById.get(summary.anchorMessageId)
        if (rawIndex === undefined) continue

        const blockRef = formatBlockRef(summary.blockId) // "bN"
        if (!lookup.has(blockRef)) {
            lookup.set(blockRef, {
                kind: "compressed-block",
                rawIndex,
                blockId: summary.blockId,
                anchorMessageId: summary.anchorMessageId,
            })
        }
    }

    return lookup
}
```

## 4.2 Boundary resolution with validation

```typescript
// lib/compress/search.ts
export function resolveBoundaryIds(
    context: SearchContext,
    state: SessionState,
    startId: string,
    endId: string,
): { startReference: BoundaryReference; endReference: BoundaryReference } {
    const lookup = buildBoundaryLookup(context, state)
    const parsedStartId = parseBoundaryId(startId)
    const parsedEndId = parseBoundaryId(endId)

    // Validate both exist
    if (parsedStartId === null) throw new Error("startId is invalid. Use mNNNN or bN.")
    if (parsedEndId === null) throw new Error("endId is invalid. Use mNNNN or bN.")

    const startReference = lookup.get(parsedStartId.ref)
    const endReference = lookup.get(parsedEndId.ref)

    if (!startReference) throw new Error(`startId ${parsedStartId.ref} not in context.`)
    if (!endReference) throw new Error(`endId ${parsedEndId.ref} not in context.`)

    // Validate order
    if (startReference.rawIndex > endReference.rawIndex)
        throw new Error(`startId appears after endId.`)

    return { startReference, endReference }
}
```

## 4.3 Selection resolution (gathering message/tool IDs in range)

```typescript
// lib/compress/search.ts
export function resolveSelection(
    context: SearchContext,
    startReference: BoundaryReference,
    endReference: BoundaryReference,
): SelectionResolution {
    const startRawIndex = startReference.rawIndex
    const endRawIndex = endReference.rawIndex

    const messageIds: string[] = []
    const toolIds: string[] = []
    const requiredBlockIds: number[] = [] // existing blocks whose anchors fall in range
    const messageTokenById = new Map<string, number>()

    // Collect all messages and tools in the range
    for (let index = startRawIndex; index <= endRawIndex; index++) {
        const rawMessage = context.rawMessages[index]
        if (!rawMessage) continue
        if (isIgnoredUserMessage(rawMessage)) continue

        const messageId = rawMessage.info.id
        messageIds.push(messageId)
        messageTokenById.set(messageId, countAllMessageTokens(rawMessage))

        // Extract tool call IDs from parts
        for (const part of rawMessage.parts) {
            if (part.type === "tool" && part.callID) {
                toolIds.push(part.callID)
            }
        }
    }

    // Find compressed blocks whose anchors are within selection
    const selectedMessageIds = new Set(messageIds)
    for (const summary of context.summaryByBlockId.values()) {
        if (selectedMessageIds.has(summary.anchorMessageId)) {
            requiredBlockIds.push(summary.blockId)
        }
    }

    return {
        startReference,
        endReference,
        messageIds,
        messageTokenById,
        toolIds,
        requiredBlockIds,
    }
}
```

## 4.4 Non-overlapping validation

```typescript
// lib/compress/range-utils.ts
export function validateNonOverlapping(plans: ResolvedRangeCompression[]): void {
    const sortedPlans = [...plans].sort(
        (left, right) =>
            left.selection.startReference.rawIndex - right.selection.startReference.rawIndex ||
            left.selection.endReference.rawIndex - right.selection.endReference.rawIndex ||
            left.index - right.index,
    )

    const issues: string[] = []
    for (let index = 1; index < sortedPlans.length; index++) {
        const previous = sortedPlans[index - 1]
        const current = sortedPlans[index]

        // No overlap if current starts after previous ends
        if (current.selection.startReference.rawIndex > previous.selection.endReference.rawIndex) {
            continue
        }

        issues.push(
            `content[${previous.index}] overlaps content[${current.index}]. ` +
                `Overlapping ranges cannot be compressed in the same batch.`,
        )
    }

    if (issues.length > 0) throw new Error(issues.join("\n"))
}
```
