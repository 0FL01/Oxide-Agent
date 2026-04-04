# 5. Block Placeholder Nesting

> Source: `lib/compress/range-utils.ts`

---

## 5.1 Parsing (bN) placeholders from model's summary

```typescript
// lib/compress/range-utils.ts
const BLOCK_PLACEHOLDER_REGEX = /\(b(\d+)\)|\{block_(\d+)\}/gi

export function parseBlockPlaceholders(summary: string): ParsedBlockPlaceholder[] {
    const placeholders: ParsedBlockPlaceholder[] = []
    const regex = new RegExp(BLOCK_PLACEHOLDER_REGEX)

    let match: RegExpExecArray | null
    while ((match = regex.exec(summary)) !== null) {
        const full = match[0]
        const blockIdPart = match[1] || match[2]
        const parsed = Number.parseInt(blockIdPart, 10)
        if (!Number.isInteger(parsed)) continue

        placeholders.push({
            raw: full,
            blockId: parsed,
            startIndex: match.index,
            endIndex: match.index + full.length,
        })
    }

    return placeholders
}
```

## 5.2 Expanding placeholders into stored summaries

```typescript
// lib/compress/range-utils.ts
export function injectBlockPlaceholders(
    summary: string,
    placeholders: ParsedBlockPlaceholder[],
    summaryByBlockId: Map<number, CompressionBlock>,
    startReference: BoundaryReference,
    endReference: BoundaryReference,
): InjectedSummaryResult {
    let cursor = 0
    let expanded = summary
    const consumed: number[] = []
    const consumedSeen = new Set<number>()

    if (placeholders.length > 0) {
        expanded = ""
        for (const placeholder of placeholders) {
            const target = summaryByBlockId.get(placeholder.blockId)
            if (!target) throw new Error(`Compressed block not found: (b${placeholder.blockId})`)

            // Splice: text before placeholder + restored block summary + continue
            expanded += summary.slice(cursor, placeholder.startIndex)
            expanded += restoreSummary(target.summary) // strip header/footer
            cursor = placeholder.endIndex

            if (!consumedSeen.has(placeholder.blockId)) {
                consumedSeen.add(placeholder.blockId)
                consumed.push(placeholder.blockId)
            }
        }
        expanded += summary.slice(cursor)
    }

    // Also inject boundary summaries (if start/end are block refs)
    expanded = injectBoundarySummary(
        expanded,
        startReference,
        "start",
        summaryByBlockId,
        consumed,
        consumedSeen,
    )
    expanded = injectBoundarySummary(
        expanded,
        endReference,
        "end",
        summaryByBlockId,
        consumed,
        consumedSeen,
    )

    return { expandedSummary: expanded, consumedBlockIds: consumed }
}
```

## 5.3 Restoring a stored summary (strip wrapper)

```typescript
// lib/compress/range-utils.ts
function restoreSummary(summary: string): string {
    const headerMatch = summary.match(/^\s*\[Compressed conversation(?: section)?(?: b\d+)?\]/i)
    if (!headerMatch) return summary

    const afterHeader = summary.slice(headerMatch[0].length)
    const withoutLeadingBreaks = afterHeader.replace(/^(?:\r?\n)+/, "")
    return withoutLeadingBreaks.replace(
        /(?:\r?\n)*\[\/Compressed conversation(?: section)?\]\s*$/i,
        "",
    )
}
```
