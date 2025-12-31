# Project: Another Chat with LLM (Rust Port)

## 🏗 Project Structure (`rust-src/`)
- `src/main.rs`: Entry point, initialization, and bot startup.
- `src/bot/`: Telegram bot logic (handlers, states).
- `src/llm/`: LLM clients (Groq, Mistral, Gemini, OpenRouter) and the `LlmProvider` trait.
- `src/storage.rs`: Data storage layer (S3/R2 compatibility).
- `src/config.rs`: Configuration and environment variable loading.
- `src/utils.rs`: Helper functions (message splitting, formatting).

# Rust Development Context & Tooling Guidelines

**PREFER MCP TOOLS over shell commands** - they're faster, token-optimized, and filter noise:

**Shell fallback**: Only use shell commands when MCP tools aren't available or for operations not covered.

## 🛡️ IMPORTANT: Tool Usage & Token Economy
**DO NOT** run verbose shell commands (like `cargo metadata` or `ls -R`) unless absolutely necessary.
**ALWAYS** prefer the specialized tools provided below. They return structured, concise data designed to save context window tokens.

## 🛠️ Operational Guidelines

### 1. Project Structure & Metadata
- **Initial Context**: Use `workspace-info` immediately to understand the project topology (members, packages) without the heavy payload of `cargo metadata`.
- **Dependency Info**: Use `cargo-info [crate]` to fetch details. Avoid reading `Cargo.toml` manually.
- **Explain Errors**: If the compiler gives an error code (e.g., E0308), **always** run `rustc-explain [code]` before attempting a fix.

### 2. Building & Checking Code
- **Quick Check**: Use `cargo-check`. This is faster and cheaper than build.
- **Full Build**: Use `cargo-build` only when executables are required.
- **Testing**:
    - Use `cargo-test` for standard runs.
    - Use `cargo-hack` to verify feature flag combinations if the issue might be feature-gated.

### 3. Documentation (CRITICAL)
**Stop guessing APIs.** If you are unsure about a crate's usage:
1.  **Find Crate**: Use `search_crate` to find the correct package name.
2.  **Search Items**: Use `search_documentation_items` to find specific structs/functions (fuzzy search).
3.  **Read Docs**: Use `retrieve_documentation_page` or `retrieve_documentation_index_page`.
4.  **Fallback**: Use `retrieve_documentation_all_items` only if fuzzy search fails.

### 4. Dependency Management
- **Adding/Removing**: Use `cargo-add` and `cargo-remove`.
- **Updates**: Use `cargo-update` to bump lockfile versions.
- **Cleanup**: Use `cargo-machete` periodically to identify unused deps.

### 5. Code Quality & Security
- **Linting**: Run `cargo-clippy` before proposing final code changes.
- **Formatting**: Run `cargo-fmt`.
- **Security**: Use `cargo-deny-check` to audit licenses and advisories.

## 📝 Coding Style & Etiquette
- **Idiomatic Rust**: Prefer `Result`/`Option` combinators (`map`, `and_then`) over explicit `match` where readable.
- **Error Handling**: Use `thiserror` for libraries and `anyhow` for applications unless specified otherwise.
- **Async**: Assume `tokio` runtime unless `async-std` is present in `workspace-info`.
- **Comments**: Write doc comments (`///`) for public APIs.

## ⚡ Tool Map (Intent -> Command)
| Intent | Preferred Tool |
| :--- | :--- |
| "Does this code compile?" | `cargo-check` |
| "What features does X have?" | `cargo-info X` |
| "What is error E0xxx?" | `rustc-explain E0xxx` |
| "Clean up unused deps" | `cargo-machete` |
| "Check detailed compatibility" | `cargo-hack` |
| "Find how to use Vec" | `search_documentation_items` -> `retrieve_documentation_page` |