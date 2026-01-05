# Clippy Error Fixes Blueprint

Fix 28 clippy errors across 5 files in `rust-src/`.

> [!NOTE]
> All errors are straightforward fixes recommended by clippy itself. No external API verification needed.

---

## Phase 1: Utils — `expect_used` Lints ✅

**Goal**: Replace `expect()` calls on `Regex::new()` with `unwrap_unchecked()` + safety comment, or use `#[allow(clippy::expect_used)]` directive.

**Resource Context**:
- 📄 `src/utils.rs` (lines 4-17)

**Steps**:
1. [ ] Add `#[allow(clippy::expect_used)]` above each `static` Regex declaration. This is safe because these are compile-time constant regex patterns verified by tests.
2. [ ] Run `cargo-check` to verify compilation.

---

## Phase 2: Preprocessor Fixes ✅

**Goal**: Fix 10 lints in `preprocessor.rs`: `uninlined_format_args`, `cast_precision_loss`, `branches_sharing_code`.

**Resource Context**:
- 📄 `src/agent/preprocessor.rs`

**Steps**:
1. [ ] **Line 117**: Inline `safe_name` → `format!("/workspace/uploads/{safe_name}")`.
2. [ ] **Line 132**: Add `#[allow(clippy::cast_precision_loss)]` above the function or on the specific line (acceptable precision for human-readable size display).
3. [ ] **Line 148**: Inline `mime` → `format!("   Тип: {mime}")`.
4. [ ] **Lines 154-160**: Refactor `branches_sharing_code` by moving `parts.push(String::new())` before the `if let`.
5. [ ] **Line 156**: Inline `msg` → `format!("**Сообщение:** {msg}")`.
6. [ ] **Lines 190, 192**: Add `#[allow(clippy::cast_precision_loss)]` to `format_file_size` function (acceptable for human-readable sizes).
7. [ ] **Line 194**: Inline `bytes` → `format!("{bytes} B")`.
8. [ ] Run `cargo-check`.

---

## Phase 3: Bot Handlers Fixes ✅

**Goal**: Fix 3 lints in `handlers.rs` and `agent_handlers.rs`.

**Resource Context**:
- 📄 `src/bot/handlers.rs`
- 📄 `src/bot/agent_handlers.rs`

**Steps**:
1. [ ] **`agent_handlers.rs` line 348**: Replace `doc.mime_type.as_ref().map(|m| m.to_string())` with `doc.mime_type.as_ref().map(ToString::to_string)`.
2. [ ] **`handlers.rs` lines 595-608**: Refactor `match state` → `if let State::AgentMode = state { ... } else { ... }`.
3. [ ] **`handlers.rs` line 597**: Wrap the `handle_agent_message` future in `Box::pin(...)` to fix `large_futures`.
4. [ ] Run `cargo-check`.

---

## Phase 4: Config Docs Fixes ✅

**Goal**: Fix 4 `doc_markdown` lints in `config.rs`.

**Resource Context**:
- 📄 `src/config.rs`

**Steps**:
1. [ ] **Line 27**: `/// ZeroAI API key` → `/// \`ZeroAI\` API key`.
2. [ ] **Line 31**: `/// OpenRouter API key` → `/// \`OpenRouter\` API key`.
3. [ ] **Line 45**: `/// Site URL for OpenRouter identification` → `/// Site URL for \`OpenRouter\` identification`.
4. [ ] **Line 48**: `/// Site name for OpenRouter identification` → `/// Site name for \`OpenRouter\` identification`.
5. [ ] Run `cargo-check`.

---

## Phase 5: Sandbox Manager Fixes ✅

**Goal**: Fix 4 lints in `sandbox/manager.rs`: `map_unwrap_or`, `uninlined_format_args`.

**Resource Context**:
- 📄 `src/sandbox/manager.rs`

**Steps**:
1. [ ] **Lines 352-355**: Replace `.map(|p| ...).unwrap_or_else(|| ...)` with `.map_or_else(|| "/workspace".to_string(), |p| p.to_string_lossy().to_string())`.
2. [ ] **Lines 356-359**: Replace `.map(|n| ...).unwrap_or_else(|| ...)` with `.map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string())`.
3. [ ] **Line 362**: Inline `parent` → `format!("mkdir -p '{parent}'")`.
4. [ ] **Line 495**: Inline `e` → `anyhow!("Failed to parse uploads size: {e}")`.
5. [ ] Run `cargo-check`.

---

## Phase 6: Final Verification ✅

**Goal**: Ensure all clippy errors are resolved.

**Steps**:
1. [ ] Run `cargo-fmt` for formatting.
2. [ ] Run `cargo-clippy` — expect **0 errors**.
3. [ ] Run `cargo-test` to ensure no regressions.

---

## Verification Plan

### Automated Tests
```bash
# Run from rust-src/
cargo fmt
cargo clippy
cargo test
```

### Expected Results
- `cargo clippy` exits with **0 errors**, **0 warnings** from `-D warnings`.
- All existing tests pass.
