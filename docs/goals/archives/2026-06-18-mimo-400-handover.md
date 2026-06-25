# Handover Note: MiMo 400 "text is not set" — Still Unresolved

Date: 2026-06-18
Branch: `feature/chrome-agent`

## Status

**UNRESOLVED.** Three serialization fixes applied and deployed; all produce the identical 400 error from MiMo's BYOK provider on iteration 3. The `reasoning_content` fix (commit `a69c7c63`) is confirmed deployed in Docker (image built 19:26 UTC, error at 19:32 UTC) but does not resolve the issue.

## Commits (all on `feature/chrome-agent`)

| Commit | Description |
|--------|-------------|
| `1defaedd` | CP1 — `artifact_uri` to attachment structs |
| `d0df3016` | CP2 — browser provider passes `artifact_uri` |
| `49e4cd46` | CP3 — runner 3-tier image resolution |
| `5264ec25` | CP4 — `content: null` for tool-only assistant messages |
| `a69c7c63` | CP5 — `reasoning_content` on tool-call assistant messages |

## What Was Tried (All Failed With Same Error)

The error is always: `400 Bad Request - {"error":{"code":"400","message":"Param Incorrect","param":"\`text\` is not set","type":""}}` from `provider_name: "Xiaomi"`, `is_byok: true`.

| Variant | Wire format | Result |
|---------|-------------|--------|
| Pre-CP4 | `"content": ""` | 400 "text is not set" |
| CP4 (`5264ec25`) | `"content": null` | 400 "text is not set" |
| CP5 (`a69c7c63`) | `"content": null` + `"reasoning_content": ""` | 400 "text is not set" |

All three produce the **identical** error. The `param` is named `text` — not `content`, not `reasoning_content`. No field called `text` exists in the OpenAI Chat Completions API spec. MiMo's BYOK provider likely maps `content` → `text` internally.

## Iteration Sequence (from live test logs)

```
iteration=1: success, content_len=0, tool_calls=1
iteration=2: success, content_len=0, tool_calls=1
iteration=3: 400 "text is not set" → Task failed
```

- Iterations 1-2: LLM returns tool_calls with empty content. Succeeds.
- Iteration 3: Request includes 2 prior tool-call assistant messages + 2 tool results. Fails.

## Critical Finding: Trace Logs NOT Visible

The `trace!` full request body logging added at `opencode_go.rs:798-804` is **not visible** because `docker-compose.web.yml` sets:

```yaml
RUST_LOG=oxide_agent_core=info,...
```

Trace level is below info. **Without trace logs, we cannot see the actual request body** to identify which message/field triggers the error.

## Key Code Locations

### Assistant message serialization (the fix that didn't work)
- `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs:441-514` — `assistant_message()` with `content: null` + `reasoning_content: ""` when `has_tool_calls && require_reasoning_content`
- `crates/oxide-agent-core/src/llm/providers/chat_completions/profile.rs` — `require_reasoning_content_on_tool_calls: bool` field, `true` for `opencode_go()`/`opencode_zen()`

### Tool result serialization (UNTESTED — potential second culprit)
- `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs:517-565` — `tool_result_message()`
- Line 553-554: when `result.content` is empty and no image parts, sends `"content": ""` — MiMo may reject this with "text is not set"
- Line 527-552: when `allow_native_image_parts=true` and `content_parts` non-empty, sends array `[{type: "text", ...}, {type: "image_url", ...}]` — MiMo may not support array content on tool messages

### Image support for MiMo
- `discovery.rs:552,638` — `mimo-v2.5` (non-pro) has `supports_image_input=true`
- `opencode_go.rs:929` — `opencode_chat_request_options()` sets `allow_native_image_parts = supports_image_input_for_model_id(model_id)` → **true for mimo-v2.5**
- This means tool results with browser screenshots are sent as multimodal arrays to MiMo

### Trace logging (needs trace level to activate)
- `opencode_go.rs:798-804` — `trace!(request_body = %event.body, "OpenCode request body")`

## Hypotheses for Next Session (Priority Order)

### H1: Enable trace logging, re-run, examine request body
**Most fundamental.** We're fixing blind. Change `RUST_LOG` in `docker-compose.web.yml` to:
```
RUST_LOG=oxide_agent_core=trace,oxide_agent_core::agent_latency=off,oxide_agent_transport_web=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn,async_openai=warn
```
Or more targeted:
```
RUST_LOG=oxide_agent_core=info,oxide_agent_core::llm::providers=trace,...
```
Rebuild, re-run test, examine the full request body JSON on iteration 3. Identify exactly which message and field triggers "text is not set".

### H2: Omit `content` field entirely from JSON
Not `null`, not `""`, but **absent** from the JSON object when `has_tool_calls && content.is_empty()`. MiMo may require the field to be absent (not present with null/empty value). This is the one variant not yet tested.

Implementation: in `assistant_message()`, conditionally construct the JSON:
```rust
let mut message = json!({"role": "assistant"});
if !msg.content.is_empty() || !has_tool_calls {
    message["content"] = content;  // string or null
}
// else: content key omitted entirely
```

### H3: Tool result empty content
`tool_result_message()` (request.rs:553-554) sends `"content": ""` for empty tool results. If browser tools return empty text content (image-only results), MiMo may reject the tool message with "text is not set".

Fix: ensure tool results always have non-empty text, or send `content: null` / omit field for empty tool results too.

### H4: Tool result array content (multimodal)
With `allow_native_image_parts=true` for MiMo, tool results with images are sent as `[{type: "image_url", ...}]` arrays. MiMo's BYOK provider may not support array content on `role: "tool"` messages — only string content.

Fix: disable `allow_native_image_parts` for MiMo, or stringify image tool results to text summaries.

### H5: MiMo requires non-empty content string
MiMo may simply require `content` to be a non-empty string on all assistant messages, regardless of tool_calls. Neither `""` nor `null` satisfies this.

Fix: use `content: " "` (single space) or a placeholder like `"."` when content is empty but tool_calls present.

## External Research Summary

- **GitHub Issue #44** (XiaomiMiMo/MiMo): `content: null` + `tool_calls` → 400. MiMo rejects standard OpenAI tool calling format. Issue still open.
- **OpenClaw Issue #81419**: MiMo/DeepSeek require `reasoning_content` on tool-call assistant messages. Fix: `requiresReasoningContentOnAssistantMessages = true`. **This is what CP5 implemented — but it didn't help.**
- **Hermes-Wiki**: mentions `_tool_result_content_for_active_model` returning text summary to avoid 400 "text is not set" — **points to tool result messages, not assistant messages.**

The Hermes-Wiki hint is significant: it specifically mentions tool result content as the cause of "text is not set", not assistant message content. This aligns with H3/H4.

## Docker / Environment

```yaml
# docker-compose.web.yml current RUST_LOG (NEEDS trace for diagnosis):
RUST_LOG=oxide_agent_core=info,oxide_agent_core::agent_latency=${OXIDE_AGENT_LATENCY_LOG:-off},oxide_agent_transport_web=info,oxide_agent_transport_web::web_latency=${OXIDE_WEB_LATENCY_LOG:-off},oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn,async_openai=warn
```

Postgres:
```
OXIDE_DATABASE_URL=postgres://oxide_agent:REDACTED@REDACTED-HOST:REDACTED-PORT/oxide_agent?sslmode=require
```
No `psql` locally; use:
```bash
docker run --rm postgres:16 psql "$OXIDE_DATABASE_URL" -c "..."
```

## Gates (All Pass)

```bash
cargo fmt --all -- --check
cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings
cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings
cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown -- -D warnings
cargo test -p oxide-agent-core --no-default-features --features profile-full --lib
cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local
cargo test -p oxide-agent-web-ui
docker compose -f docker-compose.web.yml build
```

## Recommended Next Steps

1. **Enable trace logging** (H1) — change `RUST_LOG` in docker-compose.web.yml, rebuild, re-run test.
2. **Examine the full request body** on iteration 3 — identify which message has the problematic field.
3. **Based on findings**, apply the targeted fix (H2/H3/H4/H5).
4. **If trace logs confirm the assistant message is the culprit**: try omitting `content` field entirely (H2).
5. **If trace logs point to tool result messages**: fix `tool_result_message()` — either ensure non-empty text content (H3) or disable multimodal arrays for MiMo (H4).
6. Run gates, commit, rebuild Docker, re-run live test.
