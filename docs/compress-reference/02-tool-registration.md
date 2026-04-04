# 2. Tool Registration & Schema

> Source: `lib/compress/range.ts`, `lib/compress/message.ts`, `index.ts`

---

## 2.1 Range mode tool definition

```typescript
// lib/compress/range.ts
function buildSchema() {
    return {
        topic: tool.schema
            .string()
            .describe("Short label (3-5 words) for display - e.g., 'Auth System Exploration'"),
        content: tool.schema
            .array(
                tool.schema.object({
                    startId: tool.schema
                        .string()
                        .describe(
                            "Message or block ID marking the beginning of range (e.g. m0001, b2)",
                        ),
                    endId: tool.schema
                        .string()
                        .describe("Message or block ID marking the end of range (e.g. m0012, b5)"),
                    summary: tool.schema
                        .string()
                        .describe("Complete technical summary replacing all content in range"),
                }),
            )
            .describe(
                "One or more ranges to compress, each with start/end boundaries and a summary",
            ),
    }
}
```

## 2.2 Message mode tool definition

```typescript
// lib/compress/message.ts
function buildSchema() {
    return {
        topic: tool.schema
            .string()
            .describe(
                "Short label (3-5 words) for the overall batch - e.g., 'Closed Research Notes'",
            ),
        content: tool.schema
            .array(
                tool.schema.object({
                    messageId: tool.schema
                        .string()
                        .describe("Raw message ID to compress (e.g. m0001)"),
                    topic: tool.schema
                        .string()
                        .describe("Short label (3-5 words) for this one message summary"),
                    summary: tool.schema
                        .string()
                        .describe("Complete technical summary replacing that one message"),
                }),
            )
            .describe("Batch of individual message summaries to create in one tool call"),
    }
}
```

## 2.3 Mode selection at registration time

```typescript
// index.ts
tool: {
    ...(config.compress.permission !== "deny" && {
        compress:
            config.compress.mode === "message"
                ? createCompressMessageTool(compressToolContext)
                : createCompressRangeTool(compressToolContext),
    }),
},
```
