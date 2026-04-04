# 3. Compression Pipeline (execute flow)

> Source: `lib/compress/pipeline.ts`, `lib/compress/range.ts`

---

## 3.1 Prepare: fetch messages, run strategies

```typescript
// lib/compress/pipeline.ts
export async function prepareSession(
    ctx: ToolContext,
    toolCtx: RunContext,
    title: string,
): Promise<PreparedSession> {
    if (ctx.state.manualMode && ctx.state.manualMode !== "compress-pending") {
        throw new Error(
            "Manual mode: compress blocked. Do not retry until `<compress triggered manually>` appears in user context.",
        )
    }

    // 1. Ask user permission
    await toolCtx.ask({
        permission: "compress",
        patterns: ["*"],
        always: ["*"],
        metadata: {},
    })

    toolCtx.metadata({ title })

    // 2. Fetch raw session messages
    const rawMessages = await fetchSessionMessages(ctx.client, toolCtx.sessionID)

    // 3. Ensure state is initialized
    await ensureSessionInitialized(
        ctx.client,
        ctx.state,
        toolCtx.sessionID,
        ctx.logger,
        rawMessages,
        ctx.config.manualMode.enabled,
    )

    // 4. Assign stable message refs (m0001, m0002, ...)
    assignMessageRefs(ctx.state, rawMessages)

    // 5. Run automatic strategies
    deduplicate(ctx.state, ctx.logger, ctx.config, rawMessages)
    purgeErrors(ctx.state, ctx.logger, ctx.config, rawMessages)

    return {
        rawMessages,
        searchContext: buildSearchContext(ctx.state, rawMessages),
    }
}
```

## 3.2 Finalize: persist state, send notification

```typescript
// lib/compress/pipeline.ts
export async function finalizeSession(
    ctx: ToolContext,
    toolCtx: RunContext,
    rawMessages: WithParts[],
    entries: NotificationEntry[],
    batchTopic: string | undefined,
): Promise<void> {
    ctx.state.manualMode = ctx.state.manualMode ? "active" : false
    await saveSessionState(ctx.state, ctx.logger)

    const params = getCurrentParams(ctx.state, rawMessages, ctx.logger)
    const sessionMessageIds = rawMessages
        .filter((msg) => !isIgnoredUserMessage(msg))
        .map((msg) => msg.info.id)

    await sendCompressNotification(
        ctx.client,
        ctx.logger,
        ctx.config,
        ctx.state,
        toolCtx.sessionID,
        entries,
        batchTopic,
        sessionMessageIds,
        params,
    )
}
```

## 3.3 Range mode: full execute handler

```typescript
// lib/compress/range.ts — inside createCompressRangeTool
async execute(args, toolCtx) {
    const input = args as CompressRangeToolArgs
    validateArgs(input)

    // PHASE 1: Prepare
    const { rawMessages, searchContext } = await prepareSession(
        ctx, toolCtx, `Compress Range: ${input.topic}`,
    )

    // PHASE 2: Resolve ranges (boundary IDs -> actual messages)
    const resolvedPlans = resolveRanges(input, searchContext, ctx.state)
    validateNonOverlapping(resolvedPlans)

    const notifications: NotificationEntry[] = []
    const preparedPlans = []
    let totalCompressedMessages = 0

    // PHASE 3: For each range, prepare summary with protected content
    for (const plan of resolvedPlans) {
        const parsedPlaceholders = parseBlockPlaceholders(plan.entry.summary)
        const missingBlockIds = validateSummaryPlaceholders(
            parsedPlaceholders,
            plan.selection.requiredBlockIds,
            plan.selection.startReference,
            plan.selection.endReference,
            searchContext.summaryByBlockId,
        )

        // 3a. Expand (bN) references into actual summaries
        const injected = injectBlockPlaceholders(
            plan.entry.summary,
            parsedPlaceholders,
            searchContext.summaryByBlockId,
            plan.selection.startReference,
            plan.selection.endReference,
        )

        // 3b. Append protected user messages (if configured)
        const summaryWithUsers = appendProtectedUserMessages(
            injected.expandedSummary,
            plan.selection,
            searchContext,
            ctx.state,
            ctx.config.compress.protectUserMessages,
        )

        // 3c. Append protected tool outputs
        const summaryWithTools = await appendProtectedTools(
            ctx.client, ctx.state, ctx.config.experimental.allowSubAgents,
            summaryWithUsers, plan.selection, searchContext,
            ctx.config.compress.protectedTools, ctx.config.protectedFilePatterns,
        )

        // 3d. Append any missing nested block summaries the model forgot
        const completedSummary = appendMissingBlockSummaries(
            summaryWithTools,
            missingBlockIds,
            searchContext.summaryByBlockId,
            injected.consumedBlockIds,
        )

        preparedPlans.push({
            entry: plan.entry,
            selection: plan.selection,
            anchorMessageId: plan.anchorMessageId,
            finalSummary: completedSummary.expandedSummary,
            consumedBlockIds: completedSummary.consumedBlockIds,
        })
    }

    // PHASE 4: Allocate IDs and apply state
    const runId = allocateRunId(ctx.state)

    for (const preparedPlan of preparedPlans) {
        const blockId = allocateBlockId(ctx.state)
        const storedSummary = wrapCompressedSummary(blockId, preparedPlan.finalSummary)
        const summaryTokens = countTokens(storedSummary)

        const applied = applyCompressionState(
            ctx.state,
            {
                topic: input.topic,
                batchTopic: input.topic,
                startId: preparedPlan.entry.startId,
                endId: preparedPlan.entry.endId,
                mode: "range",
                runId,
                compressMessageId: toolCtx.messageID,
                summaryTokens,
            },
            preparedPlan.selection,
            preparedPlan.anchorMessageId,
            blockId,
            storedSummary,
            preparedPlan.consumedBlockIds,
        )

        totalCompressedMessages += applied.messageIds.length

        notifications.push({
            blockId, runId,
            summary: preparedPlan.finalSummary,
            summaryTokens,
        })
    }

    // PHASE 5: Finalize
    await finalizeSession(ctx, toolCtx, rawMessages, notifications, input.topic)

    return `Compressed ${totalCompressedMessages} messages into ${COMPRESSED_BLOCK_HEADER}.`
}
```
