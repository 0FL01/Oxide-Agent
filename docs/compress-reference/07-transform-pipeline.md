# 7. Message Transform Pipeline (prune)

> Source: `lib/hooks.ts`, `lib/messages/prune.ts`, `lib/messages/utils.ts`, `lib/shared-utils.ts`

---

## 7.1 Transform handler (10-step pipeline)

```typescript
// lib/hooks.ts — createChatMessageTransformHandler
return async (input: {}, output: { messages: WithParts[] }) => {
    await checkSession(client, state, logger, output.messages, config.manualMode.enabled)
    syncCompressPermissionState(state, config, hostPermissions, output.messages)

    if (state.isSubAgent && !config.experimental.allowSubAgents) return

    // STEP 1:  Remove hallucinated DCP tags from model output
    stripHallucinations(output.messages)

    // STEP 2:  Cache system prompt token count
    cacheSystemPromptTokens(state, output.messages)

    // STEP 3:  Assign stable refs (m0001, m0002, ...) to messages
    assignMessageRefs(state, output.messages)

    // STEP 4:  Sync block state (deactivate blocks whose origin msg is gone)
    syncCompressionBlocks(state, logger, output.messages)

    // STEP 5:  Sync tool parameter cache
    syncToolCache(state, config, logger, output.messages)

    // STEP 6:  Build ordered list of all tool call IDs
    buildToolIdList(state, output.messages)

    // STEP 7:  PRUNE — replace compressed ranges, prune tool outputs/inputs
    prune(state, logger, config, output.messages)

    // STEP 8:  Inject subagent results (if enabled)
    await injectExtendedSubAgentResults(
        client,
        state,
        logger,
        output.messages,
        config.experimental.allowSubAgents,
    )

    // STEP 9:  Build priority map and inject compress nudges
    const compressionPriorities = buildPriorityMap(config, state, output.messages)
    prompts.reload()
    injectCompressNudges(
        state,
        config,
        logger,
        output.messages,
        prompts.getRuntimePrompts(),
        compressionPriorities,
    )

    // STEP 10: Inject message ID tags + priorities into message parts
    injectMessageIds(state, config, output.messages, compressionPriorities)

    // STEP 11: Apply pending manual trigger (if any)
    applyPendingManualTrigger(state, output.messages, logger)

    // STEP 12: Strip stale metadata
    stripStaleMetadata(output.messages)

    // STEP 13: Save context to disk for debugging
    if (state.sessionId) {
        await logger.saveContext(state.sessionId, output.messages)
    }
}
```

## 7.2 Prune function (dispatches to sub-handlers)

```typescript
// lib/messages/prune.ts
export const prune = (
    state: SessionState,
    logger: Logger,
    config: PluginConfig,
    messages: WithParts[],
): void => {
    filterCompressedRanges(state, logger, config, messages)
    pruneToolOutputs(state, logger, messages)
    pruneToolInputs(state, logger, messages)
    pruneToolErrors(state, logger, messages)
}
```

## 7.3 filterCompressedRanges — THE core pruning logic

```typescript
// lib/messages/prune.ts
const PRUNED_TOOL_OUTPUT_REPLACEMENT =
    "[Output removed to save context - information superseded or no longer needed]"
const PRUNED_TOOL_ERROR_INPUT_REPLACEMENT = "[input removed due to failed tool call]"
const PRUNED_QUESTION_INPUT_REPLACEMENT = "[questions removed - see output for user's answers]"

const filterCompressedRanges = (
    state: SessionState,
    logger: Logger,
    config: PluginConfig,
    messages: WithParts[],
): void => {
    if (
        state.prune.messages.byMessageId.size === 0 &&
        state.prune.messages.activeByAnchorMessageId.size === 0
    )
        return

    const result: WithParts[] = []

    for (const msg of messages) {
        const msgId = msg.info.id

        // CHECK 1: Is there a summary to inject at this anchor point?
        const blockId = state.prune.messages.activeByAnchorMessageId.get(msgId)
        const summary =
            blockId !== undefined ? state.prune.messages.blocksById.get(blockId) : undefined

        if (summary) {
            if (
                summary.active !== true ||
                typeof (summary as any).summary !== "string" ||
                (summary as any).summary.length === 0
            ) {
                logger.warn("Skipping malformed compress summary", {
                    anchorMessageId: msgId,
                    blockId: (summary as any).blockId,
                })
            } else {
                // Find last user message for variant/metadata
                const msgIndex = messages.indexOf(msg)
                const userMessage = getLastUserMessage(messages, msgIndex)

                if (userMessage) {
                    const userInfo = userMessage.info as UserMessage
                    const summaryContent =
                        config.compress.mode === "message"
                            ? replaceBlockIdsWithBlocked(rawSummaryContent) // sanitize (bN) refs
                            : rawSummaryContent

                    const summarySeed = `${summary.blockId}:${summary.anchorMessageId}`

                    // Create synthetic user message with the compressed summary
                    result.push(
                        createSyntheticUserMessage(
                            userMessage,
                            summaryContent,
                            userInfo.variant,
                            summarySeed,
                        ),
                    )
                }
            }
        }

        // CHECK 2: Skip messages that are in the prune list
        const pruneEntry = state.prune.messages.byMessageId.get(msgId)
        if (pruneEntry && pruneEntry.activeBlockIds.length > 0) {
            continue // THIS MESSAGE IS REMOVED FROM CONTEXT
        }

        // CHECK 3: Normal message — keep it
        result.push(msg)
    }

    // Replace messages array in-place
    messages.length = 0
    messages.push(...result)
}
```

## 7.4 Synthetic user message creation

```typescript
// lib/messages/utils.ts
const SUMMARY_ID_HASH_LENGTH = 16

const generateStableId = (prefix: string, seed: string): string => {
    const hash = createHash("sha256").update(seed).digest("hex").slice(0, SUMMARY_ID_HASH_LENGTH)
    return `${prefix}_${hash}`
}

export const createSyntheticUserMessage = (
    baseMessage: WithParts,
    content: string,
    variant?: string,
    stableSeed?: string,
): WithParts => {
    const userInfo = baseMessage.info as UserMessage
    const now = Date.now()
    const deterministicSeed = stableSeed?.trim() || userInfo.id
    const messageId = generateStableId("msg_dcp_summary", deterministicSeed)
    const partId = generateStableId("prt_dcp_summary", deterministicSeed)

    return {
        info: {
            id: messageId,
            sessionID: userInfo.sessionID,
            role: "user" as const,
            agent: userInfo.agent,
            model: userInfo.model,
            time: { created: now },
            ...(variant !== undefined && { variant }),
        },
        parts: [
            {
                id: partId,
                sessionID: userInfo.sessionID,
                messageID: messageId,
                type: "text" as const,
                text: content,
            },
        ],
    }
}
```

## 7.5 Tool output/input pruning

```typescript
// lib/messages/prune.ts
const pruneToolOutputs = (state, logger, messages) => {
    for (const msg of messages) {
        if (isMessageCompacted(state, msg)) continue
        for (const part of msg.parts) {
            if (part.type !== "tool") continue
            if (!state.prune.tools.has(part.callID)) continue
            if (part.state.status !== "completed") continue
            // Never prune edit/write/question outputs
            if (part.tool === "question" || part.tool === "edit" || part.tool === "write") continue

            part.state.output = PRUNED_TOOL_OUTPUT_REPLACEMENT
        }
    }
}

const pruneToolErrors = (state, logger, messages) => {
    for (const msg of messages) {
        if (isMessageCompacted(state, msg)) continue
        for (const part of msg.parts) {
            if (part.type !== "tool") continue
            if (!state.prune.tools.has(part.callID)) continue
            if (part.state.status !== "error") continue

            // Replace all string inputs with placeholder
            const input = part.state.input
            if (input && typeof input === "object") {
                for (const key of Object.keys(input)) {
                    if (typeof input[key] === "string") {
                        input[key] = PRUNED_TOOL_ERROR_INPUT_REPLACEMENT
                    }
                }
            }
        }
    }
}
```

## 7.6 isMessageCompacted — the gate check

```typescript
// lib/shared-utils.ts
export const isMessageCompacted = (state: SessionState, msg: WithParts): boolean => {
    // Created before the platform's own compaction event
    if (msg.info.time.created < state.lastCompaction) return true
    // Has at least one active compression block
    const pruneEntry = state.prune.messages.byMessageId.get(msg.info.id)
    if (pruneEntry && pruneEntry.activeBlockIds.length > 0) return true
    return false
}
```
