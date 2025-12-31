# Project: Another Chat with LLM (Rust Port)

## ⚡ Rust Development (via MCP `rust-mcp-server`)

**PREFER MCP TOOLS over shell commands** - they're faster, token-optimized, and filter noise:

| Operation | MCP Tool | Why MCP is better |
|-----------|----------|-------------------|
| **Run** | `Bash(cargo run)` | Full output for debugging |
| **Test** | `cargo-test` | Cleaner output, `no_run: true` option |
| **Lint** | `cargo-clippy` | Use `no_deps: true` for speed |
| **Format** | `Bash(cargo fmt)` | Standard formatting |
| **Check** | `cargo-check` | Quick compilation check |
| **Clean** | `Bash(cargo clean)` | Full cleanup when needed |

**Shell fallback**: Only use shell commands when MCP tools aren't available or for operations not covered.

## 🏗 Project Structure (`rust-src/`)
- `src/main.rs`: Entry point, initialization, and bot startup.
- `src/bot/`: Telegram bot logic (handlers, states).
- `src/llm/`: LLM clients (Groq, Mistral, Gemini, OpenRouter) and the `LlmProvider` trait.
- `src/storage.rs`: Data storage layer (S3/R2 compatibility).
- `src/config.rs`: Configuration and environment variable loading.
- `src/utils.rs`: Helper functions (message splitting, formatting).

## 🧠 Rules and Tools (from GEMINI.md)

### 🚫 STRICT PROHIBITIONS
1. **DO NOT HALLUCINATE APIs:** If unsure about a method signature or type name — use `rust-mcp-server` or `mcp-rust-docs` MCP tools.
2. **DO NOT USE** shell/bash commands for `cargo` (e.g., `cargo build`, `cargo test`, `cargo clippy`) if MCP `rust-mcp-server` tools are available.
3. **DO NOT READ** `Cargo.lock` or huge `cargo-metadata` outputs directly.

### 🛠 MCP TOOL REFERENCE

| Category | Tool | When to use | Token Optimization |
|-----------|------------|--------------------|---------------------|
| **Docs** | `search_crate` | Search for crate alternatives | Outputs top 10 by relevance |
| | `search_documentation_items` | Search for a specific method/type | **Best way** to avoid reading the whole doc |
| | `retrieve_documentation_page` | Deep dive into a specific API | Use only when `path` is found |
| **Core** | `cargo-check` | Quick compilation check | Use `package: ["name"]` |
| | `cargo-test` | Run tests | `no_run: true` to check test logic only |
| **Quality** | `cargo-clippy` | Find bugs | `no_deps: true` is critical for speed |
| | `workspace-info` | Structure overview | **ALWAYS** instead of reading files manually |

### 💡 TOKEN SAVING STRATEGY
1. **Multi-step search:** `search_documentation_items` -> `retrieve_documentation_page`.
2. **Relevance:** Use `version: "latest"` unless otherwise specified.
3. **Locality:** Limit scope in `check` and `clippy`.