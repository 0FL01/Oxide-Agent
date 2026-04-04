# 9. Strategies: Deduplication

> Source: `lib/strategies/deduplication.ts`

---

```typescript
// lib/strategies/deduplication.ts
export const deduplicate = (
    state: SessionState,
    logger: Logger,
    config: PluginConfig,
    messages: WithParts[],
): void => {
    if (!config.strategies.deduplication.enabled) return

    const allToolIds = state.toolIdList
    const unprunedIds = allToolIds.filter((id) => !state.prune.tools.has(id))
    if (unprunedIds.length === 0) return

    const protectedTools = config.strategies.deduplication.protectedTools

    // Group by signature: tool::sortedJSON(params)
    const signatureMap = new Map<string, string[]>()
    for (const id of unprunedIds) {
        const metadata = state.toolParameters.get(id)
        if (!metadata) continue

        // Skip protected tools and file patterns
        if (isToolNameProtected(metadata.tool, protectedTools)) continue
        const filePaths = getFilePathsFromParameters(metadata.tool, metadata.parameters)
        if (isFilePathProtected(filePaths, config.protectedFilePatterns)) continue

        const signature = createToolSignature(metadata.tool, metadata.parameters)
        if (!signatureMap.has(signature)) signatureMap.set(signature, [])
        signatureMap.get(signature)!.push(id)
    }

    // Keep only the LAST (most recent) occurrence per signature
    const newPruneIds: string[] = []
    for (const [, ids] of signatureMap.entries()) {
        if (ids.length > 1) {
            newPruneIds.push(...ids.slice(0, -1))
        }
    }

    if (newPruneIds.length > 0) {
        state.stats.totalPruneTokens += getTotalToolTokens(state, newPruneIds)
        for (const id of newPruneIds) {
            const entry = state.toolParameters.get(id)
            state.prune.tools.set(id, entry?.tokenCount ?? 0)
        }
    }
}

// Signature creation: normalize + sort keys + stringify
function createToolSignature(tool: string, parameters?: any): string {
    if (!parameters) return tool
    const normalized = normalizeParameters(parameters) // strip undefined/null
    const sorted = sortObjectKeys(normalized) // recursive key sort
    return `${tool}::${JSON.stringify(sorted)}`
}

function normalizeParameters(params: any): any {
    if (typeof params !== "object" || params === null) return params
    if (Array.isArray(params)) return params // arrays NOT sorted
    const normalized: any = {}
    for (const [key, value] of Object.entries(params)) {
        if (value !== undefined && value !== null) normalized[key] = value
    }
    return normalized
}

function sortObjectKeys(obj: any): any {
    if (typeof obj !== "object" || obj === null) return obj
    if (Array.isArray(obj)) return obj.map(sortObjectKeys)
    const sorted: any = {}
    for (const key of Object.keys(obj).sort()) {
        sorted[key] = sortObjectKeys(obj[key])
    }
    return sorted
}
```
