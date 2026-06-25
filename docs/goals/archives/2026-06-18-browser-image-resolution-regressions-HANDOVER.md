# Handover Note: Browser Image Resolution + MiMo 400 Regression

Date: 2026-06-18
Branch: `feature/chrome-agent`
Goal doc: `docs/goals/2026-06-18-browser-image-resolution-regressions.md`

## Status

CP1-CP4 complete and committed. All gates green (fmt, clippy, tests, Docker build).
CP5 (live test) **BLOCKED** — MiMo 400 "text is not set" error persists despite CP4 fix.

## Commits

- `1defaedd` CP1 — `artifact_uri` to `AgentMessageAttachment` + `ToolOutputImageAttachment`
- `d0df3016` CP2 — browser provider passes `frame.artifact.uri` into attachment
- `49e4cd46` CP3 — runner 3-tier image resolution + `StorageProvider` threading
- `5264ec25` CP4 — `content: null` for tool-only assistant messages

## What Was Fixed

### Bug 1: "Skipping native image attachment" (CP1-CP3)

**Root cause:** After the previous goal (screenshots → Postgres BYTEA), inline `data: Option<Vec<u8>>` on `AgentMessageAttachment` is `#[serde(skip, default)]` — lost after checkpoint save/load. Runner had no `StorageProvider` access and no `artifact_uri` to look up images from Postgres. Filesystem fallback failed because CP5 of previous goal stopped writing to disk.

**Fix:** 3-tier image resolution in `attach_native_image_parts_from_refs` (`llm_calls.rs:114`):
1. `attachment.data` (inline bytes, pre-checkpoint)
2. `storage.load_browser_artifact(user_id, artifact_uri)` (Postgres BYTEA, post-checkpoint)
3. `image_reader.read_native_image_file(sandbox_path)` (filesystem, legacy)

`artifact_uri: Option<String>` added to both attachment structs (serializable, survives checkpoint). `StorageProvider` threaded into `AgentRunnerContext` via `PreparedExecution::build_runner_context`.

### Bug 2: MiMo 400 "text is not set" (CP4)

**Root cause:** `assistant_message()` in `chat_completions/request.rs:429` always sent `"content": msg.content` as a String. When the LLM returns tool calls with no text (`content_len=0`), wire format was `"content": ""`. MiMo (Xiaomi BYOK provider) rejects `"content": ""` for tool-only assistant messages — it expects `"content": null`.

**Fix:** When `msg.content.is_empty() && !msg.tool_calls.is_empty()`, send `"content": Value::Null` instead of `"content": ""`.

## The Blocker

The CP4 fix is deployed in Docker (container built after commit `5264ec25`), but the live test still fails with the same 400 error on iteration 3.

**Iteration sequence from logs:**
- Iteration 0: `content_len=140 tool_calls=1` → success
- Iteration 1: `content_len=108 tool_calls=1` → success
- Iteration 2: `content_len=0 tool_calls=1` → success (empty content assistant message stored)
- Iteration 3: 400 "Param Incorrect: `text` is not set" → Task failed

The fix should have made `assistant_message()` send `content: null` for the iteration 2 message when it's replayed on iteration 3. The code path was traced end-to-end and the fix SHOULD work — but the error persists.

## Key Hypotheses (Not Yet Tested)

1. **MiMo may reject `content: null` too** — it may need the `content` field to be ABSENT from the JSON entirely, not just `null`. The OpenAI spec says `null` is valid, but MiMo's BYOK provider may not handle it.

2. **The problem may be in a different message** — not the assistant message, but possibly a tool result message with empty content, or a user message. Need full request body logging to identify which message triggers the error.

3. **There may be a second request builder path** — need to verify that `build_tool_chat_body` in `opencode_go.rs:873` is the only path that builds messages for MiMo. The `assistant_message()` fix only covers `chat_completions/request.rs` — if there's another serializer, it wouldn't have the fix.

4. **The `content` field might need to be omitted, not null** — `serde_json::json!({"role": "assistant", "tool_calls": [...]})` without a `content` key at all.

## Recommended Next Steps

1. **Add temporary debug logging** to dump the full request JSON body before sending to MiMo. Current `log_request_summary` (`opencode_go.rs:769`) only logs metadata (message count, body byte length). Add a `trace!` or `debug!` with the full `json_body` to see exactly what's being sent.

2. **Rebuild Docker, re-run live test**, inspect which message has `content: ""` or `content: null`.

3. **If `content: null` is confirmed in the body but MiMo still rejects**: try omitting the `content` field entirely from the JSON for tool-only assistant messages (use conditional `serde_json::json!` construction).

4. **If a different message is the culprit**: fix that message type too. Candidates: tool result messages with empty content (`tool_result_message` in `request.rs:469`), user messages with empty content.

5. **Remove debug logging after the fix is confirmed**.

## Key Files

- `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs:429-467` — `assistant_message()` with the CP4 fix
- `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:873-894` — `build_tool_chat_body` delegates to shared builder
- `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:769-796` — `log_request_summary` (metadata only, needs full body logging)
- `crates/oxide-agent-core/src/agent/runner/llm_calls.rs:108-199` — `attach_native_image_parts_from_refs` with 3-tier resolution
- `crates/oxide-agent-core/src/agent/runner/response_dispatch.rs:25-30` — `handle_llm_response` extracts `raw_json` from response
- `crates/oxide-agent-core/src/agent/runner/tools.rs:120-128` — `BufferedRuntimeHistory::record_assistant_turn` stores empty content
- `crates/oxide-agent-core/src/agent/runner/tools.rs:320` — `apply_buffered_runtime_history` creates `AgentMessage` with empty content
- `crates/oxide-agent-core/src/agent/memory.rs:73-93` — `AgentMessageAttachment` with `artifact_uri` field
- `crates/oxide-agent-core/src/agent/tool_runtime/output.rs` — `ToolOutputImageAttachment` with `artifact_uri` field
- `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs:83-113` — `screenshot_image_attachment` sets `artifact_uri`

## Postgres Connection

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
cargo clippy --workspace --no-default-features --features profile-full --all-targets -- -D warnings
cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings
cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown -- -D warnings
cargo test -p oxide-agent-core --no-default-features --features profile-full --lib
cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local
cargo test -p oxide-agent-web-ui
docker compose -f docker-compose.web.yml build
```
