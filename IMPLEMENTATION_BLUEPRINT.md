# Implementation Blueprint: Clippy Refactoring & Code Hardening

**Project Goal**: Resolve all `cargo clippy` errors (blocking compilation/CI) and warnings (code quality) to ensure a stable, safe, and idiomatic Rust codebase.

## Phase 1: Critical Fixes (Compilation & Panics)

**Goal**: Fix blocking errors (exit code 101) and eliminate potential runtime panics caused by unsafe `unwrap()` usage.

**Resource Context**:
- 📄 `src/storage.rs`
- 📄 `src/agent/providers/sandbox.rs`
- 📄 `src/agent/session.rs`
- 📄 `src/utils.rs`
- 📚 **Docs**: `std::marker::Sync`, `std::sync::LazyLock`, `std::result::Result`

**🛡 Invariant Check (Safety Bounds)**:
1. **Thread Safety**: Async futures returned by storage methods MUST be `Send`. Generic parameters holding references across await points must be `Sync`.
2. **Panic Freedom**: Initialization of static regexes MUST NOT panic implicitly in production code (use explicit `expect` or handle errors).
3. **Option Safety**: `unwrap()` on `Option` is strictly forbidden in logic flow (use `if let`, `match`, or `map_or`).

**Steps**:
1. [ ] **Verify Thread Safety**: Analyze `src/storage.rs` to confirm that adding `Sync` bound to `save_json<T>` generic solves the `future cannot be sent between threads` error.
2. [ ] **Fix Storage Concurrency**: Update `save_json` signature in `src/storage.rs` to require `T: serde::Serialize + Sync`.
3. [ ] **Fix Sandbox Safety**: Refactor `ensure_sandbox` in `src/agent/providers/sandbox.rs` and `src/agent/session.rs` to use pattern matching (`if let`) instead of `is_none()` + `unwrap()`.
4. [ ] **Modernize Lazy Statics**:
    - Verify `std::sync::LazyLock` availability (Rust 1.80+).
    - Refactor `src/utils.rs` to replace `lazy_static!` macros with `std::sync::LazyLock`.
    - Replace `unwrap()` in regex compilation with `expect("valid regex pattern")` to satisfy linter while maintaining distinct panic messages.
5. [ ] **QA**: Run `cargo check` to ensure the project compiles without errors.

## Phase 2: Performance & Resource Safety

**Goal**: Prevent stack overflows from large futures and deadlocks from held mutexes.

**Resource Context**:
- 📄 `src/bot/handlers.rs`
- 📄 `src/agent/executor.rs`
- 📄 `src/agent/providers/sandbox.rs`
- 📄 `src/agent/memory.rs`
- 📚 **Docs**: `std::boxed::Box`, `std::pin::Pin`, `tokio::sync::Mutex`

**Steps**:
1. [ ] **Verify Future Sizes**: Identify specific calls in `src/bot/handlers.rs` causing `large_futures` warning (>20KB).
2. [ ] **Heap Allocation**: Wrap `handle_agent_message` and `check_state_and_redirect` calls in `Box::pin(...)` within `src/bot/handlers.rs`.
3. [ ] **Lock Scoping**: Refactor `src/agent/executor.rs` and `src/agent/providers/sandbox.rs` to strictly scope `MutexGuard` lifetimes (using explicit blocks `{}` or `drop()`) before `.await` points to prevent deadlocks and silence `significant_drop_tightening`.
4. [ ] **Safe Arithmetic**: Rewrite percentage calculations in `src/agent/memory.rs`. Replace unsafe `as f64` -> `as usize` casts with integer arithmetic (e.g., `(len * 20) / 100`) to avoid precision loss and truncation warnings.

## Phase 3: Idiomatic Rust & Code Quality

**Goal**: Resolve remaining warnings (formatting, attributes, documentation) to reach "Zero Warnings".

**Resource Context**:
- 📄 All source files
- 📚 **Docs**: `std::fmt`, `clippy::uninlined_format_args`

**Steps**:
1. [ ] **Format Strings**: Batch apply `uninlined_format_args` (change `format!("v: {}", v)` to `format!("v: {v}")`) across all files.
2. [ ] **Attributes**:
    - Add `#[must_use]` to getters and pure functions (e.g., `new`, `is_running`).
    - Convert eligible functions to `const fn`.
3. [ ] **Documentation**:
    - Add missing backticks to doc comments (e.g., refer to \`Struct\` instead of Struct).
    - Add `# Errors` section to doc comments for functions returning `Result`.
4. [ ] **Logic Simplification**:
    - Replace `match` with `if let` where applicable.
    - Replace `option.map(...).unwrap_or(...)` with `option.map_or(...)`.
    - Remove redundant closures (e.g., `.map(|s| s.to_string())` -> `.map(ToString::to_string)`).
5. [ ] **Final QA**: Run `cargo clippy --all-targets --all-features` to confirm zero warnings.

[!NOTE]
Do not remove `#[allow(...)]` attributes if they are protecting intentionally unused code that is planned for future features.
