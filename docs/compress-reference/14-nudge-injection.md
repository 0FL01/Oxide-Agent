# 14. Nudge Injection

> Source: `lib/messages/inject/inject.ts`

---

## 14.1 Compress nudge injection

```typescript
// lib/messages/inject/inject.ts
export const injectCompressNudges = (
    state: SessionState,
    config: PluginConfig,
    logger: Logger,
    messages: WithParts[],
    prompts: RuntimePrompts,
    compressionPriorities?: CompressionPriorityMap,
): void => {
    if (compressPermission(state, config) === "deny") return
    if (state.manualMode) return

    // Skip if last assistant message already contains a compress call
    const lastAssistantMessage = messages.findLast(m => m.info.role === "assistant")
    if (lastAssistantMessage && messageHasCompress(lastAssistantMessage)) {
        state.nudges.contextLimitAnchors.clear()
        state.nudges.turnNudgeAnchors.clear()
        state.nudges.iterationNudgeAnchors.clear()
        return
    }

    // Check context limits
    const { overMaxLimit, overMinLimit } = isContextOverLimits(config, state, ...)

    if (overMaxLimit) {
        // CONTEXT-LIMIT nudge: context exceeds max threshold
        addAnchor(state.nudges.contextLimitAnchors, ...)
    } else if (overMinLimit) {
        // TURN-BOUNDARY nudge: at user/assistant turn boundary while over min
        if (isLastMessageUser && lastAssistantMessage) {
            state.nudges.turnNudgeAnchors.add(lastMessage.info.id)
            state.nudges.turnNudgeAnchors.add(lastAssistantMessage.info.id)
        }

        // ITERATION-WARNING nudge: too many assistant turns since last user message
        const messagesSinceUser = countMessagesAfterIndex(messages, lastUserMessageIndex)
        if (messagesSinceUser >= iterationThreshold) {
            addAnchor(state.nudges.iterationNudgeAnchors, ...)
        }
    }

    applyAnchoredNudges(state, config, messages, prompts, compressionPriorities)
}
```

## 14.2 Message ID injection

```typescript
// Inject message ID tags into message parts
export const injectMessageIds = (
    state: SessionState,
    config: PluginConfig,
    messages: WithParts[],
    compressionPriorities?: CompressionPriorityMap,
): void => {
    for (const message of messages) {
        if (isIgnoredUserMessage(message)) continue

        const messageRef = state.messageIds.byRawId.get(message.info.id)
        if (!messageRef) continue

        const isBlockedMessage = isProtectedUserMessage(config, message)
        const priority =
            config.compress.mode === "message" && !isBlockedMessage
                ? compressionPriorities?.get(message.info.id)?.priority
                : undefined

        // Tag: <dcp-message-id ref="m0001" />
        //   or: <dcp-message-id ref="BLOCKED" />  (for protected messages)
        const tag = formatMessageIdTag(
            isBlockedMessage ? "BLOCKED" : messageRef,
            priority ? { priority } : undefined,
        )

        // Inject into last tool part, or last text part, or create synthetic part
        // (exact injection strategy depends on message role and part types)
    }
}
```
