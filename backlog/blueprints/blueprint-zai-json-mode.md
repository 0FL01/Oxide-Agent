# Blueprint: Z.AI Native JSON Mode for Agent Loop

**Issue**: Agent loop takes 15+ minutes for simple tasks due to structured output parsing failures (19 continuation loops, 8+ validation errors).

**Root Cause**: Z.AI LLM ignores JSON schema in prompt instructions. Native `response_format: { type: "json_object" }` is supported but not used.

**Expected Result**: 5-10x speedup, near-zero structured output failures.

---

## Phase 1: Enable Native JSON Mode in ZaiProvider [ ]

**Goal**: Add `response_format: { type: "json_object" }` to Z.AI API requests for agent completions.

**Resource Context**:
- 📄 `src/llm/providers/zai.rs` - Z.AI provider implementation
- 📄 `src/llm/mod.rs` - LlmProvider trait definition
- 📄 `src/llm/common.rs` - shared types
- 📚 **Docs**: `backlog/docs/zai/structure-output.md` - Z.AI JSON mode documentation

**Steps**:
1. [ ] **Analyze trait**: Read `LlmProvider` trait to understand current method signatures
2. [ ] **Extend trait**: Add optional `json_mode: bool` parameter to `chat_with_tools` or create dedicated `agent_completion` method
3. [ ] **Implement in ZaiProvider**: Add `response_format: { type: "json_object" }` to request body when json_mode=true
4. [ ] **Update callers**: Modify agent executor to pass json_mode=true for agent requests
5. [ ] **QA**: Run `cargo-check` to verify compilation

> [!IMPORTANT]
> Z.AI does NOT support streaming with OpenAI crate. All agent requests are non-streaming.
> Only modify `ZaiProvider` - do not touch other providers (Gemini, OpenRouter, etc.)

---

## Phase 2: Simplify Structured Output Validation [ ]

**Goal**: Reduce recovery complexity since native JSON mode guarantees valid JSON.

**Resource Context**:
- 📄 `src/agent/structured_output.rs` - parsing and validation logic
- 📄 `src/agent/recovery.rs` - malformed response recovery

**Steps**:
1. [ ] **Simplify recovery**: Remove or deprecate `extract_fenced_json()` and markdown fence handling - not needed with native JSON mode
2. [ ] **Keep schema validation**: Retain validation for `thought`, `tool_call`, `final_answer` structure
3. [ ] **Add metrics**: Log when recovery fallbacks are triggered (should be rare now)
4. [ ] **QA**: Run `cargo-test --package oxide_agent` for structured_output tests

> [!NOTE]
> Recovery mechanisms remain as fallback for edge cases, but should rarely trigger with native JSON mode.

---

## Phase 3: Relax Completion Hook [ ]

**Goal**: Allow early exit when LLM provides valid `final_answer`, even with incomplete todos.

**Resource Context**:
- 📄 `src/agent/hooks/completion.rs` - completion check hook
- 📄 `src/agent/hooks/types.rs` - HookContext, HookResult types
- 📄 `src/agent/runner/responses.rs` - response handling

### 3.1 Modify Completion Hook Logic

**Current logic**:
```
if !todos.is_complete() → ForceIteration (always)
```

**New logic**:
```
if has_valid_final_answer → Continue (allow exit)
if !todos.is_complete() && no_final_answer → ForceIteration
```

**Steps**:
1. [ ] **Extend HookEvent**: Add `has_final_answer: bool` field to `AfterAgent` variant
2. [ ] **Update hook logic**: Check for final_answer before forcing continuation
3. [ ] **Pass context**: Modify `handle_final_response()` to include final_answer status in hook event

### 3.2 Add Structured Output Failure Limit

**Goal**: Fail-fast after N consecutive structured output errors instead of 19 retries.

**Steps**:
1. [ ] **Add counter**: Track `structured_output_failures` in `RunState` (separate from `continuation_count`)
2. [ ] **Set limit**: After 3-5 consecutive failures, accept raw response as final_answer with warning
3. [ ] **Reset counter**: Reset on successful parse
4. [ ] **QA**: Run `cargo-check` and `cargo-test`

---

## Phase 4: Reduce AGENT_CONTINUATION_LIMIT [ ]

**Goal**: Reduce maximum forced continuations as fail-safe.

**Resource Context**:
- 📄 `src/config.rs` - configuration constants

**Steps**:
1. [ ] **Reduce limit**: Change `AGENT_CONTINUATION_LIMIT` from 20 to 8-10
2. [ ] **QA**: Run `cargo-check`

---

## Verification Checklist [ ]

After all phases complete:

1. [ ] Run `cargo-check --all-targets`
2. [ ] Run `cargo-test`
3. [ ] Run `cargo-clippy`
4. [ ] Manual test: Send agent task that previously caused 15+ min execution
5. [ ] Verify logs show 0-2 continuations instead of 19

---

## Risk Assessment

| Risk | Mitigation |
|------|------------|
| Z.AI JSON mode changes response quality | Keep schema validation, monitor output quality |
| Breaking change in LlmProvider trait | Add optional parameter with default=false for backward compat |
| Todos left incomplete on early exit | Log warning, user can see incomplete todos in UI |

---

## Files to Modify

| File | Phase | Change Type |
|------|-------|-------------|
| `src/llm/mod.rs` | 1 | Trait extension |
| `src/llm/providers/zai.rs` | 1 | Add response_format |
| `src/agent/executor.rs` | 1 | Pass json_mode=true |
| `src/agent/structured_output.rs` | 2 | Simplify recovery |
| `src/agent/hooks/completion.rs` | 3.1 | Relax exit condition |
| `src/agent/hooks/types.rs` | 3.1 | Extend HookEvent |
| `src/agent/runner/responses.rs` | 3.2 | Add failure limit |
| `src/agent/runner/types.rs` | 3.2 | Add failure counter |
| `src/config.rs` | 4 | Reduce limit |
