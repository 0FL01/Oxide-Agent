# 10. Strategies: Purge Errors

> Source: `lib/strategies/purge-errors.ts`

---

```typescript
// lib/strategies/purge-errors.ts
export const purgeErrors = (
    state: SessionState,
    logger: Logger,
    config: PluginConfig,
    messages: WithParts[],
): void => {
    if (!config.strategies.purgeErrors.enabled) return

    const allToolIds = state.toolIdList
    const unprunedIds = allToolIds.filter((id) => !state.prune.tools.has(id))
    if (unprunedIds.length === 0) return

    const protectedTools = config.strategies.purgeErrors.protectedTools
    const turnThreshold = Math.max(1, config.strategies.purgeErrors.turns)

    const newPruneIds: string[] = []
    for (const id of unprunedIds) {
        const metadata = state.toolParameters.get(id)
        if (!metadata) continue

        if (isToolNameProtected(metadata.tool, protectedTools)) continue
        const filePaths = getFilePathsFromParameters(metadata.tool, metadata.parameters)
        if (isFilePathProtected(filePaths, config.protectedFilePatterns)) continue

        if (metadata.status !== "error") continue // only errored tools

        const turnAge = state.currentTurn - metadata.turn
        if (turnAge >= turnThreshold) newPruneIds.push(id) // old enough to prune
    }

    if (newPruneIds.length > 0) {
        state.stats.totalPruneTokens += getTotalToolTokens(state, newPruneIds)
        for (const id of newPruneIds) {
            const entry = state.toolParameters.get(id)
            state.prune.tools.set(id, entry?.tokenCount ?? 0)
        }
    }
}
```
