# Project: Another Chat with LLM (Rust Port)

## ⚡ Commands
- **Run**: `cd rust-src && cargo run`
- **Test**: `cd rust-src && cargo test`
- **Lint**: `cd rust-src && cargo clippy`
- **Format**: `cd rust-src && cargo fmt`
- **Check**: `cd rust-src && cargo check`
- **Clean**: `cd rust-src && cargo clean`

## 🏗 Project Structure (`rust-src/`)
- `src/main.rs`: Entry point, initialization, and bot startup.
- `src/bot/`: Telegram bot logic (handlers, states).
- `src/llm/`: LLM clients (Groq, Mistral, Gemini, OpenRouter) and the `LlmProvider` trait.
- `src/storage.rs`: Data storage layer (S3/R2 compatibility).
- `src/config.rs`: Configuration and environment variable loading.
- `src/utils.rs`: Helper functions (message splitting, formatting).

## 🧠 Rules and Tools (from GEMINI.md)

### 🚫 STRICT PROHIBITIONS
1. **DO NOT HALLUCINATE APIs:** If unsure about a method signature or type name — use `mcp-rust-docs`.
2. **DO NOT USE** shell/bash commands for `cargo` (e.g., `cargo build`) if it is possible to use native tools (although `run_command` in the terminal is often used for `run` and `test`).
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