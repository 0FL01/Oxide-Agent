# 12. Message ID Assignment & Formatting

> Source: `lib/message-ids.ts`

---

```typescript
// lib/message-ids.ts

const MESSAGE_REF_WIDTH = 4 // m0001, m0002, ...
const MESSAGE_REF_MAX_INDEX = 9999

export function formatMessageRef(index: number): string {
    return `m${index.toString().padStart(MESSAGE_REF_WIDTH, "0")}`
}

export function formatBlockRef(blockId: number): string {
    return `b${blockId}`
}

export function formatMessageIdTag(
    ref: string,
    attributes?: Record<string, string | undefined>,
): string {
    // Produces: <dcp-message-id ref="m0001" ... />
    const serializedAttributes = Object.entries(attributes || {})
        .sort(([l], [r]) => l.localeCompare(r))
        .map(([name, value]) => {
            if (!name.trim() || typeof value !== "string" || !value.length) return ""
            return ` ${name}="${escapeXmlAttribute(value)}"`
        })
        .join("")

    return `<dcp-message-id ref="${ref}"${serializedAttributes}>\n`
}

// Assign stable refs to all messages in order
export function assignMessageRefs(state: SessionState, messages: WithParts[]): number {
    let assigned = 0
    for (const message of messages) {
        if (isIgnoredUserMessage(message)) continue

        const rawMessageId = message.info.id
        if (typeof rawMessageId !== "string" || !rawMessageId.length) continue

        // Reuse existing ref if message was seen before
        const existingRef = state.messageIds.byRawId.get(rawMessageId)
        if (existingRef) {
            state.messageIds.byRef.set(existingRef, rawMessageId)
            continue
        }

        // Allocate next ref: m0001, m0002, ...
        const ref = allocateNextMessageRef(state)
        state.messageIds.byRawId.set(rawMessageId, ref)
        state.messageIds.byRef.set(ref, rawMessageId)
        assigned++
    }
    return assigned
}

function allocateNextMessageRef(state: SessionState): string {
    let candidate = Math.max(1, state.messageIds.nextRef)
    while (candidate <= 9999) {
        const ref = formatMessageRef(candidate)
        if (!state.messageIds.byRef.has(ref)) {
            state.messageIds.nextRef = candidate + 1
            return ref
        }
        candidate++
    }
    throw new Error("Message ID alias capacity exceeded")
}
```
