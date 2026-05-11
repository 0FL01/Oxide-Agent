# 16. End-to-End Examples

---

## 16.1 Range compression: before

```
m0001 [user]    "Investigate the auth system"
m0002 [assistant] "I'll check auth.ts..."
m0003 [tool]     Read("auth.ts") → 200 lines
m0004 [assistant] "Found bug on line 42..."
m0005 [tool]     Edit("auth.ts") → fixed 3 functions
m0006 [user]    "Good. Now the dashboard."
m0007 [assistant] "Starting dashboard..."
```

## 16.2 Model calls compress

```json
{
    "topic": "Auth Fix",
    "content": [
        {
            "startId": "m0001",
            "endId": "m0005",
            "summary": "Fixed auth bugs: (1) Added expiry check to validateToken(), (2) Added mutex to createSession(). Modified auth.ts."
        }
    ]
}
```

## 16.3 Range compression: after (next LLM request)

```
[user] [Compressed conversation section]
Fixed auth bugs: (1) Added expiry check to validateToken(), (2) Added mutex to createSession(). Modified auth.ts.
[/Compressed conversation section]


[user] "Good. Now the dashboard."          ← m0006 untouched
[assistant] "Starting dashboard..."         ← m0007 untouched
```

**State changes:**

- `blocksById(1)` = { active: true, anchorMessageId: "m0001", effectiveMessageIds: ["m0001".."m0005"] }
- `byMessageId("m0001".."m0005")` = { activeBlockIds: [1] } → these messages are SKIPPED
- `activeByAnchorMessageId("m0001")` = 1 → synthetic message injected here

## 16.4 Nesting: second compression overlaps first

After the first compression (b1 covers m0001..m0005), a second compression covers m0001..m0007:

```json
{
    "topic": "Auth + Dashboard",
    "content": [
        {
            "startId": "m0001",
            "endId": "m0007",
            "summary": "Fixed auth bugs (b1) and built dashboard component with charts and tables."
        }
    ]
}
```

**Processing:**

1. `parseBlockPlaceholders` finds `(b1)` in summary
2. `injectBlockPlaceholders` replaces `(b1)` with b1's stored summary
3. b1 is deactivated (consumed), b2 becomes active
4. b2's `consumedBlockIds: [1]`, b1's `parentBlockIds: [2]`

## 16.5 Deduplication example

```
Turn 1: read({path: "/foo.txt"})  → call_1
Turn 2: read({path: "/bar.txt"})  → call_2
Turn 3: read({path: "/foo.txt"})  → call_3   ← duplicate of call_1
Turn 4: read({path: "/foo.txt"})  → call_4   ← newest for this signature
```

After dedup: `call_1` and `call_3` pruned, `call_2` (unique) and `call_4` (latest) kept.

## 16.6 Purge-errors example

```
Turn 1: apply_patch({patchText: "500 lines..."}) → ERROR "conflict"
Turn 5: (current turn = 5, threshold = 4, age = 4 >= 4)
```

Result: tool input `patchText` replaced with `[input removed due to failed tool call]`, error message preserved.

---

## Key Architectural Invariants (for Rust port)

1. **Session history is IMMUTABLE.** Compression state is a separate overlay. The transform pipeline rebuilds the outgoing message array without touching stored messages.

2. **Messages are rebuilt in-place:** `messages.clear(); messages.extend(result)`. In Rust, use `&mut Vec<Message>` or return a new `Vec`.

3. **Block nesting preserves information.** Consumed blocks are deactivated but retained in state. Their summaries are expanded into the new block's summary text.

4. **Dedup and purge-errors only run at compress time**, not every turn, to minimize cache disruption.

5. **Protected content is appended, not summarized.** Protected tool outputs and user messages are verbatim additions to the summary text.

6. **State persists per session** to survive restarts. Block state, prune maps, and nudge anchors are serialized to disk.

7. **Two compression modes:** `range` (contiguous spans with nesting) and `message` (individual messages, no nesting, block refs sanitized to `BLOCKED`).

8. **Pipeline order matters:** The 10+ step transform must run in exact sequence — hallucination strip → refs → sync → prune → nudges → ID injection.
