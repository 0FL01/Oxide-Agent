# Project: Another Chat with LLM

## 🏗 Project Structure (`rust-src/`)
- `src/main.rs`: Entry point, initialization, and bot startup.
- `src/lib.rs`: Library root, module exports and shared functionality.
- `src/agent/`: Agent Mode logic (session, executor, memory, preprocessor).
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

### 3. Web Search & Documentation (Tavily)
**Stop guessing.** Use real-time data for both general info and library documentation:
1.  **Search**: Use `tavily-search` for overall context, news, or specific library docs.
2.  **Content**: Use `tavily-extract` to get clean markdown from relevant URLs.
3.  **Site Analysis**: Use `tavily-crawl` to explore site hierarchies if needed.

### 4. Dependency Management
- **Adding/Removing**: Use `cargo-add` and `cargo-remove`.
- **Updates**: Use `cargo-update` to bump lockfile versions.
- **Cleanup**: Use `cargo-machete` periodically to identify unused deps.

### 5. Code Quality & Security
- **Linting**: Run `cargo-clippy` before proposing final code changes.
- **Formatting**: MANDATORY. Run `cargo-fmt` (via `mcp:rust-mcp-server`) before ANY code submission to pass CI.
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
| "Research/Docs/Search" | `tavily-search` -> `tavily-extract` |