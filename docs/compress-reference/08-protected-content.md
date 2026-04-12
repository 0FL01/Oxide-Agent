# 8. Protected Content

> Source: `lib/compress/protected-content.ts`

---

## 8.1 Protected user messages (appended verbatim to summary)

```typescript
// lib/compress/protected-content.ts
export function appendProtectedUserMessages(
    summary: string,
    selection: SelectionResolution,
    searchContext: SearchContext,
    state: SessionState,
    enabled: boolean, // config.compress.protectUserMessages
): string {
    if (!enabled) return summary

    const userTexts: string[] = []

    for (const messageId of selection.messageIds) {
        // Skip already-compressed messages
        const entry = state.prune.messages.byMessageId.get(messageId)
        if (entry && entry.activeBlockIds.length > 0) continue

        const message = searchContext.rawMessagesById.get(messageId)
        if (!message || message.info.role !== "user") continue
        if (isIgnoredUserMessage(message)) continue

        for (const part of message.parts) {
            if (part.type === "text" && typeof part.text === "string" && part.text.trim()) {
                userTexts.push(part.text)
                break
            }
        }
    }

    if (userTexts.length === 0) return summary

    const heading = "\n\nThe following user messages were sent in this conversation verbatim:"
    return summary + heading + userTexts.map((text) => `\n${text}`).join("")
}
```

## 8.2 Protected tool outputs (appended to summary)

```typescript
// lib/compress/protected-content.ts
export async function appendProtectedTools(
    client: any,
    state: SessionState,
    allowSubAgents: boolean,
    summary: string,
    selection: SelectionResolution,
    searchContext: SearchContext,
    protectedTools: string[],              // e.g. ["task", "skill", "todowrite", "todoread"]
    protectedFilePatterns: string[] = [],
): Promise<string> {
    const protectedOutputs: string[] = []

    for (const messageId of selection.messageIds) {
        const entry = state.prune.messages.byMessageId.get(messageId)
        if (entry && entry.activeBlockIds.length > 0) continue

        const message = searchContext.rawMessagesById.get(messageId)
        if (!message) continue

        for (const part of message.parts) {
            if (part.type === "tool" && part.callID) {
                // Check protection: by tool name or by file path pattern
                let isToolProtected = isToolNameProtected(part.tool, protectedTools)

                if (!isToolProtected && protectedFilePatterns.length > 0) {
                    const filePaths = getFilePathsFromParameters(part.tool, part.state?.input)
                    if (isFilePathProtected(filePaths, protectedFilePatterns))
                        isToolProtected = true
                }

                if (isToolProtected) {
                    let output = ""
                    if (part.state?.status === "completed" && part.state.output)
                        output = typeof part.state.output === "string"
                            ? part.state.output
                            : JSON.stringify(part.state.output)

                    // Subagent expansion (if enabled)
                    if (allowSubAgents && part.tool === "task" && /* ... */) {
                        // Fetch subagent session messages and merge into output
                    }

                    if (output)
                        protectedOutputs.push(`\n### Tool: ${part.tool}\n${output}`)
                }
            }
        }
    }

    if (protectedOutputs.length === 0) return summary

    const heading = "\n\nThe following protected tools were used in this conversation as well:"
    return summary + heading + protectedOutputs.join("")
}
```
