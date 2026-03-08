# Oxide Agent TG Bot

[(Russian README)](README-ru.md)

Universal Telegram bot with AI assistant, supporting multiple models, multimodality, and advanced **Agent Mode** with code execution.

## Description

<details>
<summary>ℹ️ About: Tech Stack (Rust, Teloxide), Integrations, and Architecture</summary>

This project is a Telegram bot that integrates with various Large Language Model (LLM) APIs to provide users with a multifunctional AI assistant. The bot can process text, voice, video messages, and images, work with documents, manage dialogue history, and perform complex tasks in an isolated sandbox.

The bot is developed using **Rust 1.92**, the `teloxide` library, and integrates with 3 main AI providers for Chat/Agent mode (**z.ai**, **OpenRouter**, **Mistral**), along with Groq and Google Gemini support.
</details>

## Features

*   **🏗️ Workspace Architecture:** Modular crate design with clear separation of concerns:
    - `oxide-agent-core` - Domain logic, LLM integrations, hooks, skills, storage
    - `oxide-agent-runtime` - Session orchestration, execution cycle, tool providers, sandbox
    - `oxide-agent-transport-telegram` - Telegram transport layer (teloxide integration)
    - `oxide-agent-telegram-bot` - Binary entry point and configuration

*   **🤖 Agent Mode:**
        <img width="974" height="747" alt="image_2026-01-11_20-58-21" src="https://github.com/user-attachments/assets/c99e55e4-8933-4ec8-9f50-22f7cbca4c77" />

    *   **Integrated Sandbox:** Safe execution of Python code and Bash commands in isolated Docker containers (`debian:trixie-slim`).
    *   **Tools:** Read/write files, execute commands, web search, work with video and file hosting.
    *   **📋 Task Management (Todos):** `write_todos` system for planning and tracking progress of complex requests.
    *   **🎯 Skills System:** RAG system with embeddings to automatically provide relevant context from markdown documents (9 skills: core, delegation_manager, ffmpeg-conversion, file-hosting, file-management, html-report, task-planning, video-processing, web-search).
    *   **📁 File Handling:** Accept files from user (up to 20MB), send to Telegram (up to 50MB), or upload to cloud (up to 4GB) with link generation.
    *   **🎬 Video Processing:** `yt-dlp` integration for downloading video and media files from the internet.
        <img width="977" height="762" alt="image" src="https://github.com/user-attachments/assets/1ffb66b7-559b-453f-9330-fbe27ccee90e" />

    *   **☁️ File Hosting:** Upload files from sandbox to public hosting with short retention time.
    *   **Web Search and Data Extraction:** Tavily API or Crawl4AI integration for retrieving up-to-date information from the web (configurable via `SEARCH_PROVIDER`).
    *   **🔗 Hooks System:** Extensible architecture for intercepting and customizing agent behavior:
        - Completion Check Hook - validates task completion
        - Workload Distributor - enforces separation of duties by blocking heavy manual operations in the Main Agent
        - Search Budget Hook - prevents infinite loops in tool calls
        - Delegation Guard - controls sub-agent delegation behavior
        - Soft Timeout Report Hook - provides detailed timeout reporting
        - Sub-Agent Safety - ensures safe execution environments
        - Registry - centralized hook management
    *   **🔄 Loop Detection:** Three levels of protection (Content Detector, Tool Detector, LLM Detector) to prevent infinite loops.
    *   **⏱️ Universal Runtime:** Transport-agnostic progress rendering system that can be adapted for Discord, Slack, and other transports.
    *   **👥 Hierarchical Delegation:** The Main Agent acts as an orchestrator, delegating heavy retrieval and mechanical tasks (git clone, searching) to Sub-Agents to maximize efficiency and context preservation.
    *   **Autonomy:** Agent plans steps and selects tools itself.
    *   **Separate Authorization:** Access control to agent via `AGENT_ACCESS_IDS`.
    *   **Long-term Memory and Context:** Up to 200K tokens with automatic compression when limit is reached.
    *   **🗣️ Narrator:** Separate model for summarizing agent thoughts and actions in chat.
    *   **Execution Progress:** Interactive display of current working step in Telegram.
*   **Multi-LLM Support:** 3 providers for Chat/Agent mode (**z.ai**, **OpenRouter**, **Mistral**). Groq and Google Gemini are supported in **Chat Mode only** (Agent Mode in development).
*   **Native Tool Calling:** Efficient use of tools in modern models.
*   **Multimedia Processing:**
    *   Voice and video messages (speech recognition via Gemini).
    *   Images (analysis and description via multimodal models).
    *   Work with documents of various formats.
*   **Context Management:** Dialogue history saved in Cloudflare R2 (S3).
*   **🔒 Security and Quality:** `unsafe_code = "forbid"`, strict Clippy lints, no panics (`zero-panic profile`).

## System Requirements

<details>
<summary>🔑 API Keys and Infrastructure</summary>

### 🔑 API Keys (Mandatory)
| Provider | Variable | Description |
| :--- | :--- | :--- |
| **ZAI** | `ZAI_API_KEY` | **Mandatory for Agent** (`glm-4.7`, Default Agent Model). [Zhipu AI](https://z.ai/) |
| **Telegram** | `TELEGRAM_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) |
| **Cloudflare R2** | `R2_*` | S3 storage (Access Key, Secret, Endpoint, Bucket) |
| **Mistral AI** | `MISTRAL_API_KEY` | **Critical for Agent** (`mistral-embed` model for skill selection) |

### 🤖 Supported LLM Providers for Chat/Agent Mode
The bot supports 3 main providers for both standard chat and advanced Agent mode (with tool calling):

*   **ZAI** (`ZAI_API_KEY`) — primary provider for Agent Mode (`glm-4.7` or `glm-4.5-air`). ZAI is [Zhipu AI](https://z.ai/) and provides native tool-aware chat completions and reasoning.
*   **OpenRouter** (`OPENROUTER_API_KEY`) — commonly used for chat/multimodal requests (e.g., `google/gemini-3-flash-preview`). Supports tool calling for Agent mode through compatible models. Ensure `CHAT_MODEL_PROVIDER=openrouter` if you need Gemini voice/image support.
*   **Mistral** (`MISTRAL_API_KEY`) — great for cost-effective agent/chat combos (e.g., `mistral-large-latest`, `pixtral-large-latest`). Supports tool calling via JSON mode or native tools.

#### Other Providers (Chat only, Agent mode in development)
*   **Groq** (`GROQ_API_KEY`) — optional provider for fast specialized chat workloads (e.g. `llama-3.3-70b-versatile`).
*   **Google Gemini** (`GEMINI_API_KEY`) — direct integration for Gemini models, primarily used for multimodal tasks or as a fallback.

> [!NOTE]
> Voice recognition and image analysis depend on whichever multimodal model you configure via `CHAT_MODEL_*`/`MEDIA_MODEL_*`. The bot exposes only the models you declare in `.env`, so `Change Model` will only list those names.

### 🛠 Infrastructure
*   **Docker** — run code sandbox (`agent-sandbox:latest`)
*   **Tavily API** — optional for web search (`TAVILY_API_KEY`)
*   **Crawl4AI** — alternative deep web crawling provider with markdown extraction and PDF parsing capabilities
</details>

## Installation and Launch

<details>
<summary>🚀 Installation Instructions (Docker & Source)</summary>

1.  **Clone the repository:**
    ```bash
    git clone https://github.com/0FL01/oxide-agent.git
    cd oxide-agent
    ```

2.  **Configure environment variables:**
    Create `.env` based on `.env.example`.

3.  **Build sandbox image:**
    ```bash
    docker build -t agent-sandbox:latest -f sandbox/Dockerfile.sandbox ..
    ```

4.  **Build and run the bot:**
    ```bash
    docker-compose up --build -d
    ```
</details>

## Configuration (.env)

<details>
<summary>⚙️ Example Configuration File</summary>

```dotenv
# Telegram
TELEGRAM_TOKEN=YOUR_TOKEN
ALLOWED_USERS=ID1,ID2 # List of allowed Telegram IDs (basic access)
AGENT_ACCESS_IDS=ID1 # Access to Agent Mode (consumes many tokens)
AGENT_MODE_ENABLED=false # Stage 6 rollout flag: enable Agent Mode entrypoints gradually

# Agent Configuration
AGENT_TIMEOUT_SECS=300          # Agent execution timeout
SEARCH_PROVIDER=tavily          # Search provider (tavily/crawl4ai)
DEBUG_MODE=false                # Debug logging mode

# Cloudflare R2 (S3)
R2_ACCESS_KEY_ID=...
R2_SECRET_ACCESS_KEY=...
R2_ENDPOINT_URL=...
R2_BUCKET_NAME=...

# API Keys
GROQ_API_KEY=...
MISTRAL_API_KEY=...
GEMINI_API_KEY=...
OPENROUTER_API_KEY=...
ZAI_API_KEY=... # ZAI Provider (Zhipu AI)
TAVILY_API_KEY=... # Tavily key for web search in Agent mode (optional)
```
</details>

## Model Configuration

Set available chat/agent models through `.env`. Only declared model names appear in the bot's menus and multimodal handlers.

### Chat model (multimodal)
```dotenv
CHAT_MODEL_ID="google/gemini-3-flash-preview"
CHAT_MODEL_PROVIDER="openrouter"
CHAT_MODEL_NAME="✨ Gemini 3.0 Flash"
```
Swap `CHAT_MODEL_PROVIDER`/`CHAT_MODEL_ID` and adjust the name when you need a different multimodal provider (e.g., `mistral-large-latest`).

*   **Agent & Sub-agent (Recommended Models)**
  For the best performance in Agent Mode, it is highly recommended to use **glm-4.7** for the Main Agent and **glm-4.5-air** for the Sub-Agent (both via **ZAI** provider).
```dotenv
AGENT_MODEL_ID="glm-4.7"
AGENT_MODEL_PROVIDER="zai"

SUB_AGENT_MODEL_ID="glm-4.5-air"
SUB_AGENT_MODEL_PROVIDER="zai"
```
Omitting the sub-agent block falls back to the agent model settings.

### Optional overrides
```dotenv
MEDIA_MODEL_ID="google/gemini-3-flash-preview"
MEDIA_MODEL_PROVIDER="openrouter"

NARRATOR_MODEL_ID="labs-mistral-small-creative"
NARRATOR_MODEL_PROVIDER="mistral"
```

## Agent Mode v2 rollout safety

- Keep `AGENT_MODE_ENABLED=false` by default in production until rollout checks pass.
- Enable it gradually for supported operators/users by combining it with `AGENT_ACCESS_IDS`.
- Rollback is safe: setting `AGENT_MODE_ENABLED=false` blocks new activations but keeps runtime recovery/observation of existing tasks.
- Follow the operator checklist and support playbook in `docs/stage-6-slice-6-4-rollout.md`.

### Alternate provider example
```
CHAT_MODEL_ID="mistral-large-latest"
CHAT_MODEL_PROVIDER="mistral"

AGENT_MODEL_ID="devstral-2512"
AGENT_MODEL_PROVIDER="mistral"
```

Repeat the `_MODEL_ID/_MODEL_PROVIDER` pattern for Groq, Gemini-specific IDs, or other providers you want to expose. Only set names will be available in the chat mode keyboard.

## Available Models

| Name | Provider | Features |
| :--- | :--- | :--- |
| **OR Gemini 3 Flash** | OpenRouter | Multimodal, default chat model |
| **ZAI GLM-4.7** | ZAI (Zhipu AI) | Default agent model, GLM Coding Plan |
| **Mistral Large** | Mistral | Free and generous |
| **Gemini 2.5 Flash Lite** | Google | Cheap and efficient |
| **Devstral 2512** | Mistral | Top free for coding and agent work |

> **Note:** The models listed above are recommended configurations. Only models declared in your `.env` file will be available in the bot's "Change Model" menu.

## Agent Architecture


<details>
<summary>🏗 Internal Structure, Skills, Hooks</summary>

### 🎯 Skills System
The agent uses a RAG approach with embeddings to automatically provide relevant context:
- **9 skills** as markdown documents (`skills/`)
- **Semantic matching** of user requests with skills via cosine similarity
- **Embeddings caching** for fast access (Moka cache)
- **Automatic injection** of relevant instructions into the system prompt

### 🔄 Loop Protection
Three-level loop detection system (`agent/loop_detection/`):
1. **Content Detector** — analyzes repeating agent messages
2. **Tool Detector** — tracks identical tool calls
3. **LLM Detector** — uses LLM to analyze loop patterns

### 🔗 Hooks System
Extensible architecture for personalizing agent behavior:
- **Completion Hook** — task completion handling
- **Workload Distributor** — enforces separation of duties by blocking heavy manual operations in the Main Agent and encouraging delegation
- **Delegation Guard** — prevents delegation of high-level analytical tasks ("think", "analyze"), restricting sub-agents to mechanical retrieval
- **Sub-Agent Safety** — ensures safe execution environments for delegated tasks
- **Registry** — centralized hook management

### 🛠️ Tool Providers
The agent uses a modular provider system, each offering a specialized set of tools:
- **Sandbox Provider** (`sandbox.rs`, ~20KB) — code execution, file read/write, shell commands
- **Tavily Provider** (`tavily.rs`) — web search and data extraction
- **Crawl4AI Provider** (`crawl4ai.rs`) — deep web crawling with markdown extraction and PDF parsing
- **Todos Provider** (`todos.rs`) — task list management for long-term planning
- **YT-DLP Provider** (`ytdlp.rs`, ~33KB) — video and audio download from various platforms
- **File Hoster Provider** (`filehoster.rs`) — public file upload to temporary hosting (up to 4GB)
- **Path Provider** (`path.rs`) — path and file structure operations
- **Delegation Provider** (`delegation.rs`) — sub-agent delegation for complex task decomposition
</details>

## Usage

1.  Send `/start` to the bot.
2.  **Regular Mode:** Just write messages or send files/voice notes.
3.  **🤖 Agent Mode:** Click the "🤖 Agent Mode" button. Now the bot can execute code and use advanced tools.

<details>
<summary>💡 Agent Command Examples and Control</summary>

**Agent Command Examples:**
- *"Write a python script that downloads the google homepage and finds the word 'Search' there"*
- *"Download video from YouTube via link [URL] and convert it to MP4 via FFmpeg"*
- *"Create a CSV file with weather data and upload it to file.io"*
- *"Find information about latest AI news via web search"*

**Control:** Use "Clear Context", "Change Model" or "Extra Functions" buttons.
</details>

## Project Structure

<details>
<summary>📂 File Tree (expand)</summary>

```text
crates/
├── oxide-agent-core/           # Domain logic, LLM integrations, hooks, skills, storage
│   └── src/
│       ├── agent/              # Agent core and execution logic
│       │   ├── hooks/          # Execution hooks (Completion, Workload, Delegation, Safety)
│       │   ├── loop_detection/ # Loop detection (content, tool, llm)
│       │   ├── providers/      # Tool providers (Sandbox, Tavily, Crawl4AI, Delegation, etc.)
│       │   └── skills/         # Skills subsystem (RAG/embeddings)
│       ├── llm/                # LLM provider integrations
│       └── config.rs
├── oxide-agent-runtime/        # Session orchestration, execution cycle, tool providers, sandbox
│   └── src/
├── oxide-agent-transport-telegram/  # Telegram transport layer (teloxide integration)
│   └── src/
│       ├── handlers/           # Telegram handlers
│       └── views/              # Message templates and UI
└── oxide-agent-telegram-bot/   # Binary entry point and configuration
    └── src/
        └── main.rs

skills/                         # Skill definitions (markdown)
backlog/                        # Documentation, plans and blueprints
tests/                          # Integration and functional tests
sandbox/                        # Docker configuration for sandbox
```
</details>

## Key Dependencies

<details>
<summary>📦 Main Rust Libraries</summary>

**Main libraries:**

- **teloxide** (0.17.0) — Telegram Bot API with macros and handlers
- **tokio** (1.48) — asynchronous runtime
- **async-openai** (0.32.2) — work with OpenAI-compatible APIs
- **aws-sdk-s3** (1.119.0) — Cloudflare R2 integration
- **bollard** (0.19.4) — Docker API for sandbox management
- **reqwest** (0.12) — HTTP client with multipart and streaming support
- **serde_json** (1.0) — JSON serialization/deserialization
- **tiktoken-rs** (0.9.1) — token counting for various models
- **lazy-regex** (3.5.1) — optimized regular expressions
- **moka** (0.12) — high-performance cache with TTL
- **tavily** (2.0) — optional feature for web search
- **chrono** (0.4.42) — date and time handling
- **thiserror** (2.0.17) — custom error creation
- **anyhow** (1.0.100) — simplified error handling in application
</details>

## Development

<details>
<summary>💻 Developer Commands and CI/CD</summary>

For local development (requires Rust installed):
```bash
# Check
cargo check

# Testing (132 tests)
cargo test --release

# Linting (Clippy with warn/deny)
cargo clippy --tests -- -D warnings

# Formatting
cargo fmt

# Build with feature flags
cargo build --release --features tavily
```

### Testing Infrastructure

The project uses a comprehensive testing approach:

- **Hermetic Tests**: Isolated tests in `tests/hermetic_agent.rs` (236 lines) using mock implementations
- **Property-Based Testing**: Fuzzing tests in `tests/proptest_recovery.rs` (66 lines) for robustness validation
- **Snapshot Testing**: Regression tests in `tests/snapshot_prompts.rs` (26 lines) for prompt validation
- **Test Utilities**: Helper functions in `src/testing.rs` for quick mock setup

**Testing Dependencies:**
- `mockall` (0.14.0) - Trait-based mocking framework
- `insta` (1.46.1) - Snapshot testing framework

### CI/CD

The project uses GitHub Actions for automatic testing and deployment:
- **Testing:** Runs `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt`
- **Deployment:** Automatic deploy to server via SSH on push to `main`

### Security and Lints

- **`unsafe_code = "forbid"`** in workspace lints — unsafe code is forbidden
- **Clippy lints (forbid level):**
  - `unwrap_used = "forbid"` — all Result/Option must be handled via `?` or `match`
  - `too_many_lines = "forbid"` — files >300 lines must be split
  - `too_many_arguments = "forbid"` — functions >3 arguments require Context/Config struct
- **Feature flags:** Tavily available via `--features tavily`
- **Error Handling:** Using `thiserror` for library errors, `anyhow` for application
</details>

## License

The project is distributed under the **GNU Affero General Public License v3 (AGPL-3.0)**. Details in the [LICENSE](https://github.com/0FL01/oxide-agent/blob/main/LICENSE) file.

Copyright (C) 2026 @0FL01
