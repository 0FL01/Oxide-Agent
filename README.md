# Oxide Agent

Universal Telegram bot with AI assistant, supporting multiple models, multimodality, and advanced **Agent Mode** with code execution, plus a **Web Interface** for browser-based chat with the agent.

## Description

<details>
<summary>About: Tech Stack (Rust 1.94), Integrations, and Architecture</summary>

This project is a Telegram bot that integrates with various Large Language Model (LLM) APIs to provide users with a multifunctional AI assistant. The bot can process text, voice, video messages, and images, work with documents, manage dialogue history, and perform complex tasks in an isolated sandbox.

The bot is developed using **Rust 1.94**, the `teloxide` library, and integrates with **7 Agent Mode LLM providers**: ChatGPT/Codex (OAuth), OpenCode Go, OpenCode Zen, Zhipu AI/ZAI, MiniMax, Mistral, and OpenRouter.

### Architecture Highlights

- **Modular Workspace:** Separation into domain logic (core), orchestration (runtime), and transport layers
- **Transport-Agnostic Runtime:** Progress rendering and execution model can be adapted for Discord, Slack, etc.
- **Web Interface:** Browser-based chat with the agent (Leptos SPA, SSE streaming, dark theme, markdown rendering)
- **Topic-Scoped Infrastructure:** Per-topic agent profiles, hooks, tools, and memory isolation
- **Manager Control Plane:** Programmatic topic management with RBAC, audit trail, and rollback support
- **Sandbox Backends:** Docker broker isolation by default, with optional direct Docker access
- **Wiki Memory:** SQLx/Postgres-backed persistent memory with optional LLM-assisted extraction
- **Prompt Cache Optimization:** Static prefix + dynamic suffix assembly with validated 80%+ cache hit rate on OpenCode Go
</details>

## Features

*   **Workspace Architecture:** Modular crate design with clear separation of concerns:
    - `oxide-agent-core` - Domain logic, LLM integrations, hooks, compaction, storage
    - `oxide-agent-runtime` - Session orchestration, execution cycle, tool providers, sandbox
    - `oxide-agent-transport-telegram` - Telegram transport layer (teloxide integration)
    - `oxide-agent-transport-web` - Web interface backend (axum HTTP API, SSE, auth) and E2E test transport
    - `oxide-agent-web-contracts` - Shared web API types: auth, config, events, sessions, tasks
    - `oxide-agent-web-ui` - Web interface frontend (Leptos SPA): chat UI, SSE streaming, markdown rendering, dark theme
    - `oxide-agent-sandboxd` - Sandbox broker daemon for Docker access isolation in the default Compose deployment
    - `oxide-agent-telegram-bot` - Binary entry point and configuration

*   **Agent Mode:**

    *   **Integrated Sandbox:** Safe execution of Python code and shell commands in isolated sandbox instances. Docker/broker is the default deployment path.
    *   **Parallel Tool Execution:** Multiple tool calls in one LLM response execute concurrently for faster task completion.
    *   **Fire-and-Forget Checkpoint:** Memory persistence is async, non-blocking for reduced latency.
    *   **History Repair:** Validates tool_call_id before LLM calls; orphaned tool results prevented during compaction.
    *   **Tools:** Read/write files, execute commands, web search, work with video and file hosting.
    *   **Task Management (Todos):** `write_todos` system for planning and tracking progress of complex requests.
    *   **Durable Context:** Topic `AGENTS.md`, wiki memory, runtime injections, and enabled tools provide deterministic prompt context.
    *   **File Handling:** Accept files from user (up to 20MB), send to Telegram (up to 50MB), or upload to cloud (up to 4GB) with link generation.
    *   **Video Processing:** `yt-dlp` integration for downloading video and media files from the internet.

    *   **File Hosting:** Upload files from sandbox to public hosting with short retention time.
    *   **Web Search and Data Extraction:** Tavily, Brave Search, CRW, and local `web_markdown`/`web_crawler` handle discovery and URL-to-Markdown extraction.
    *   **Hooks System:** Extensible architecture for intercepting and customizing agent behavior:
        - Completion Check Hook - validates task completion
        - Tool Access Policy - enforces profile-level tool allowlists and blocklists
        - Hot Context Health - monitors context health during execution
        - Search Budget Hook - prevents infinite loops in tool calls
        - Soft Timeout Report Hook - provides detailed timeout reporting
        - Sub-Agent Safety - ensures safe execution environments
        - Episodic Extract / Retrieval Advisor - wiki memory integration hooks
        - Registry - centralized hook management
    *   **Universal Runtime:** Transport-agnostic progress rendering system that can be adapted for Discord, Slack, and other transports.
    *   **Hierarchical Delegation:** The Main Agent spawns async Sub-Agents for parallel, independent subtasks. Each sub-agent runs in an isolated ephemeral session with a task-specific tool whitelist, inherits the topic AGENTS.md and parent cancellation, and returns results via background job tracking.
    *   **Autonomy:** Agent plans steps and selects tools itself.
    *   **Telegram Authorization:** Access control via `TELEGRAM_ALLOWED_USERS`.
    *   **Long-term Memory and Context:** Up to 200K tokens with automatic compression when limit reached.
    *   **Execution Progress:** Interactive display of current working step in Telegram.
*   **Multi-LLM Support:** 7 Agent Mode providers: ChatGPT/Codex (OAuth), OpenCode Go, OpenCode Zen, Zhipu AI/ZAI, MiniMax, Mistral, and OpenRouter.
*   **Native Tool Calling:** Efficient use of tools in modern models with ToolCallCorrelation architecture.
*   **Web Interface:** Browser-based chat with the agent -- Leptos SPA with SSE streaming for real-time responses, dark theme, and markdown rendering.
*   **Multimedia Processing:**
    *   Voice and video messages (speech recognition via OpenRouter-hosted Gemini-family models or Voxtral).
    *   Images (analysis and description via multimodal models).
    *   Work with documents of various formats.
*   **Voice Synthesis:** Kokoro TTS for English voice replies and Silero TTS for Russian voice replies.
*   **Context Management:** Dialogue history saved in SQLx/Postgres with context-scoped isolation per topic.
*   **Wiki Memory:** Persistent SQLx/Postgres-backed memory pages with optional LLM-assisted extraction and retrieval.
*   **Prompt Cache Optimization:** Static prefix + dynamic suffix assembly order maximizes cache hit rate, with validated 80%+ cache hit on OpenCode Go.

## Screenshots

The Web Interface is a Leptos SPA with a dark theme, SSE streaming, and markdown rendering.

<p align="center">
  <img width="900" alt="Web UI empty state" src="assets/screenshots/web-ui-empty-state.jpeg" />
  <br />
  <em>New session screen with the session list, profile selector, and message input.</em>
</p>

<p align="center">
  <img width="900" alt="Web UI agent working" src="assets/screenshots/web-ui-agent-thinking.jpeg" />
  <br />
  <em>Live agent progress with the Thinking indicator and Stop control.</em>
</p>

<p align="center">
  <img width="900" alt="Web UI agent response" src="assets/screenshots/web-ui-agent-answer.jpeg" />
  <br />
  <em>Formatted agent response with tables and the Activity panel showing sub-agent execution.</em>
</p>

<p align="center">
  <img width="900" alt="Web UI settings" src="assets/screenshots/web-ui-settings.jpeg" />
  <br />
  <em>Settings page for account, default model, agent profiles, and password management.</em>
</p>

### Telegram Agent Mode

<p align="center">
  <img width="900" alt="Telegram Agent Mode overview" src="assets/screenshots/telegram-agent-mode-overview.png" />
  <br />
  <em>Agent Mode task list, tool calls, and completed file delivery in Telegram.</em>
</p>

<p align="center">
  <img width="900" alt="Telegram Agent Mode video" src="assets/screenshots/telegram-agent-mode-video.png" />
  <br />
  <em>Video download via yt-dlp and file sent back to the user in Telegram.</em>
</p>

## System Requirements

<details>
<summary>API Keys and Infrastructure</summary>

### API Keys (Mandatory)
| Provider | Variable | Description |
| :--- | :--- | :--- |
| **OpenCode Go** | `OPENCODE_GO_API_KEY` | **Primary Agent Mode provider** - recommended route: `deepseek-v4-flash` via `opencode-go`. [OpenCode](https://opencode.ai/) |
| **Telegram** | `TELEGRAM_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) |
| **PostgreSQL** | `OXIDE_DATABASE_URL` | SQLx durable storage for sessions, memory, web state, reminders, and audit |
| **Zhipu AI (ZAI)** | `OPENAI_BASE_PROVIDERS__1__*` | Configure as OpenAI Base profile `zai` for GLM routes (`glm-4.7`, `glm-4.5-air`). [Zhipu AI](https://z.ai/) |
| **Mistral AI** | `MISTRAL_API_KEY` | Required for Mistral routes (`mistral-large-latest`, etc.) |

For Supabase Postgres or small local deployments, keep the shared SQLx pool conservative (`OXIDE_DATABASE_MAX_CONNECTIONS=5`), run migrations as a deploy step, and keep the default Postgres task-file byte limit unless WAL/backups have been reviewed. `docker-compose.web.local-services.yml` includes a local Postgres on `127.0.0.1:55432`; the app image ships `/app/migrations`, and web Compose enables startup migrations by default so fresh local or single-instance remote databases cannot race web startup reconciliation.

### Supported LLM Providers for Agent Mode
The bot supports these Agent Mode provider routes/profiles with tool calling:

*   **OpenCode Go** (`OPENCODE_GO_API_KEY`) - **primary (recommended) provider for Agent Mode**. Uses subscription OpenAI-compatible API at `opencode.ai/zen/go`. Recommended Agent Mode model: `deepseek-v4-flash` with provider `opencode-go`. Supports native tool calls (strict), structured JSON for DeepSeek V4 routes, reasoning content parsing, adaptive throttling, and unbounded retry.
*   **OpenCode Zen** - Free-tier filtered variant of OpenCode Go. Same provider code, filtered to free-only models via discovery. Provider alias: `opencode-zen`.
*   **ChatGPT/Codex** (`CHATGPT_AUTH_PATH`) - Headless OAuth provider for OpenAI Codex Responses API at `chatgpt.com/backend-api/codex/responses`. SSE streaming. No audio/image support. Use `cargo run -p oxide-agent-telegram-bot --bin chatgpt-login -- login` for initial auth.
*   **Zhipu AI / ZAI** (`OPENAI_BASE_PROVIDERS__1__PROFILE=zai`) - Alternative OpenAI Base profile for Agent Mode (`glm-4.7` or `glm-4.5-air`). Provides native tool-aware chat completions and reasoning.
*   **MiniMax** (`MINIMAX_API_KEY`) - Claude SDK-compatible provider via MiniMax API (`MiniMax-M2.7`).
*   **Mistral** (`MISTRAL_API_KEY`) - Cost-effective agent routes and Voxtral audio transcription (`voxtral-mini-latest`).
*   **OpenRouter** (`OPENROUTER_API_KEY`) - Multimodal/media routes and approved tool-capable Agent Mode routes, including Gemini-family model IDs through OpenRouter.

> [!NOTE]
> Voice recognition and image analysis require an explicit `MEDIA_MODEL_ID` / `MEDIA_MODEL_PROVIDER` route.

### Infrastructure
*   **Docker** - run the default code sandbox (`agent-sandbox:latest`)
*   **Sandbox Broker** - Unix socket broker for Docker access isolation in Docker Compose (`SANDBOX_BACKEND=broker`)
*   **Tavily API** - optional web search provider (`TAVILY_API_KEY`)
*   **CRW** - optional self-hosted web search and scrape backend (`OXIDE_CRW_ENABLED`, `OXIDE_CRW_BASE_URL`)
*   **Local Web Markdown** - lightweight single-URL HTTP fetch with HTML-to-Markdown conversion and response/output limits
*   **Kokoro TTS Server** - optional for English voice message synthesis (`KOKORO_TTS_URL`)
*   **Silero TTS Server** - optional for Russian voice message synthesis (`SILERO_TTS_URL`)
*   **Wiki Memory Writer** - optional background LLM-assisted memory extraction (`WIKI_MEMORY_WRITER_ENABLED`)
</details>

## Installation and Launch

Deployment guide: [`docs/deploy.md`](docs/deploy.md).

Quick Docker start:

```bash
git clone https://github.com/0FL01/oxide-agent.git
cd oxide-agent
cp .env.example .env
$EDITOR .env
docker compose up --build -d
```

## Configuration (.env)

<details>
<summary>Example Configuration File</summary>

```dotenv
# Telegram
TELEGRAM_TOKEN=YOUR_TOKEN
TELEGRAM_ALLOWED_USERS=ID1,ID2
TELEGRAM_MANAGER_ALLOWED_USERS=ID1
ATTACH_DETACH_ENABLED=true
REMINDER_AGENT_PROGRESS_ENABLED=false
REMINDER_SILENT_NO_CHANGE_ENABLED=true

# Agent Configuration
AGENT_TIMEOUT_SECS=300
DEBUG_MODE=false

# PostgreSQL durable storage
OXIDE_DATABASE_URL=postgres://oxide_agent:oxide_agent@localhost:5432/oxide_agent
OXIDE_DATABASE_MAX_CONNECTIONS=5
OXIDE_DATABASE_MIGRATE_ON_STARTUP=false
OXIDE_WEB_TASK_FILE_MAX_BYTES=33554432

# API Keys
CHATGPT_AUTH_PATH=/app/config/chatgpt/auth.json
MISTRAL_API_KEY=...
OPENROUTER_API_KEY=...
OPENCODE_GO_API_KEY=...
OPENCODE_GO_API_BASE=https://opencode.ai/zen/go/v1/chat/completions
MINIMAX_API_KEY=...

# ZAI / GLM through OpenAI Base
OPENAI_BASE_PROVIDERS__1__NAME=zai
OPENAI_BASE_PROVIDERS__1__API_BASE=https://api.z.ai/api/coding/paas/v4
OPENAI_BASE_PROVIDERS__1__API_KEY=...
OPENAI_BASE_PROVIDERS__1__PROFILE=zai

# Web Search Providers (can be enabled together)
TAVILY_API_KEY=...
# BRAVE_SEARCH_API_KEY=...
# BRAVE_SEARCH_ENABLED=true
# Brave Search — primary indexed web discovery when BRAVE_SEARCH_API_KEY is configured.
# CRW (self-hosted web search + scrape fallback)
# OXIDE_CRW_ENABLED=true
# OXIDE_CRW_BASE_URL=http://127.0.0.1:3000
# OXIDE_CRW_API_TOKEN=...
# CRW backs `web_search` and the rendered fallback path of `web_crawler`.

# Wiki Memory Writer (background, optional LLM-assisted)
# WIKI_MEMORY_WRITER_ENABLED=true
# WIKI_MEMORY_WRITER_MODEL_ID="google/gemini-3-flash-preview"
# WIKI_MEMORY_WRITER_MODEL_PROVIDER="openrouter"
```

Plain `docker-compose.web.yml` is remote-Postgres friendly and expects `OXIDE_DATABASE_URL` from `.env` or the shell. Add `docker-compose.web.local-services.yml` when you want the bundled local Postgres. Keep `OXIDE_DATABASE_MIGRATE_ON_STARTUP=true` unless a separate migration job is guaranteed to finish before web startup.
</details>

## Model Configuration

Set explicit agent and media routes through `.env`.

*   **Agent and Sub-agent (Recommended Models)**
  For the best performance in Agent Mode, it is highly recommended to use **deepseek-v4-flash** for both the Main Agent and Sub-Agent (via **OpenCode Go** provider). This route offers strict tool calling, structured output support, reasoning content, adaptive throttling, and unlimited retry for reliable agent execution.
```dotenv
AGENT_MODEL_ID="deepseek-v4-flash"
AGENT_MODEL_PROVIDER="opencode-go"

SUB_AGENT_MODEL_ID="deepseek-v4-flash"
SUB_AGENT_MODEL_PROVIDER="opencode-go"
```

  **Alternative (ZAI):** If you prefer ZAI/GLM, configure it as an OpenAI Base `zai` profile and use **glm-4.7** for the Main Agent and **glm-4.5-air** for the Sub-Agent:
```dotenv
OPENAI_BASE_PROVIDERS__1__NAME=zai
OPENAI_BASE_PROVIDERS__1__API_BASE=https://api.z.ai/api/coding/paas/v4
OPENAI_BASE_PROVIDERS__1__API_KEY=...
OPENAI_BASE_PROVIDERS__1__PROFILE=zai

AGENT_MODEL_ID="glm-4.7"
AGENT_MODEL_PROVIDER="openai-base:zai"

SUB_AGENT_MODEL_ID="glm-4.5-air"
SUB_AGENT_MODEL_PROVIDER="openai-base:zai"
```
  **Alternative (ChatGPT/Codex):** OAuth-based provider using the Codex Responses API:
```dotenv
AGENT_MODEL_ID="gpt-5.4"
AGENT_MODEL_PROVIDER="chatgpt"
AGENT_MODEL_MAX_OUTPUT_TOKENS=32000
AGENT_MODEL_CONTEXT_WINDOW_TOKENS=128000
```
  Use `cargo run -p oxide-agent-telegram-bot --bin chatgpt-login -- login` for initial OAuth setup.

  Omitting the sub-agent block falls back to the agent model settings.

### Optional overrides
```dotenv
MEDIA_MODEL_ID="google/gemini-3.1-flash-lite-preview"
MEDIA_MODEL_PROVIDER="openrouter"
```

<details>
<summary>Weighted Model Routes (Failover)</summary>

Configure multiple weighted routes for automatic failover after persistent 429 errors:

```dotenv
# Priority: OpenCode Go (DeepSeek V4 Flash) > ZAI/OpenAI Base (GLM-4.7) > Mistral
AGENT_MODEL_ROUTES__0__ID="deepseek-v4-flash"
AGENT_MODEL_ROUTES__0__PROVIDER="opencode-go"
AGENT_MODEL_ROUTES__0__WEIGHT=10

AGENT_MODEL_ROUTES__1__ID="glm-4.7"
AGENT_MODEL_ROUTES__1__PROVIDER="openai-base:zai"
AGENT_MODEL_ROUTES__1__WEIGHT=5

AGENT_MODEL_ROUTES__2__ID="mistral-small-2603"
AGENT_MODEL_ROUTES__2__PROVIDER="mistral"
AGENT_MODEL_ROUTES__2__WEIGHT=2
```

</details>

<details>
<summary>Alternate provider example</summary>

```
AGENT_MODEL_ID="devstral-2512"
AGENT_MODEL_PROVIDER="mistral"

MEDIA_MODEL_ID="voxtral-mini-latest"
MEDIA_MODEL_PROVIDER="mistral"
```

Use `AGENT_MODEL_ROUTES__N__*` for main-agent failover and `SUB_AGENT_MODEL_ROUTES__N__*` for sub-agent failover.

</details>

## Available LLM Providers

| Provider | Description |
| :--- | :--- |
| **OpenCode Go** | Primary (recommended) agent provider, subscription OpenAI-compatible API, DeepSeek V4 Flash, strict tool calling, structured output, reasoning effort |
| **OpenCode Zen** | Free-tier variant of OpenCode Go, filtered to free-only models via discovery |
| **ChatGPT/Codex** | Headless OAuth provider for OpenAI Codex Responses API, SSE streaming, no audio/image |
| **ZAI (Zhipu AI)** | Alternative agent provider, native tool-aware chat, GLM-4.7 / GLM-4.5-Air |
| **MiniMax** | Claude SDK-compatible, high context (MiniMax-M2.7) |
| **Mistral** | Generous free tier, includes Voxtral audio transcription |
| **OpenRouter** | Aggregator for various models, including Gemini-family model IDs |

> **Note:** Gemini-family models are configured through OpenRouter routes, not a direct Google Gemini provider.

<details>
<summary>Tool Providers</summary>

### Web Search and Extraction
- **Brave Search Provider** (`tool-brave-search`) - primary indexed web discovery when `BRAVE_SEARCH_API_KEY` is configured
- **Tavily Provider** (`tool-tavily`) - web search and data extraction
- **[CRW Provider](https://github.com/us/crw)** (`tool-crw`) - self-hosted `web_search` and rendered scrape fallback for `web_crawler`
- **WebFetch Markdown Provider** (`tool-webfetch-md`) - single-URL HTTP fetch with HTML-to-Markdown conversion and context-bomb limits

### Sandbox
- **Sandbox Exec Provider** (`tool-sandbox-exec`) - command execution in sandbox
- **Sandbox File Ops Provider** (`tool-sandbox-fileops`) - file read/write/list/edit in sandbox
- **Sandbox Lifecycle Provider** (`tool-sandbox-recreate`) - sandbox recreate

### Voice Synthesis
- **Kokoro TTS Provider** (`tool-tts-kokoro`) - English voice message synthesis
- **Silero TTS Provider** (`tool-tts-silero`) - Russian voice message synthesis with SSML support

### Voice Synthesis Configuration

**Kokoro TTS (English):**

Tool: `text_to_speech_en`

Server setup: see [KOKORO-TTS-setup guide](https://github.com/0FL01/KOKORO-TTS-setup) for manual server setup.

```dotenv
KOKORO_TTS_URL=http://127.0.0.1:8000  # Default
KOKORO_TTS_VOICE=af_heart           # Default voice
KOKORO_TTS_FORMAT=ogg               # Recommended for Telegram
KOKORO_TTS_TIMEOUT_SECS=60
```

Available voices: `af_bella`, `af_aoede`, `af_alloy`, `af_heart` (default)
Formats: `ogg` (recommended), `mp3`, `wav`

**Silero TTS (Russian):**

Tool: `text_to_speech_ru`

Server setup: see [Oxide-Agent-TTS](https://github.com/0FL01/Oxide-Agent-TTS) for containerized Kokoro + Silero TTS servers with FastAPI.

```dotenv
SILERO_TTS_URL=http://127.0.0.1:8001       # Default
SILERO_TTS_SPEAKER=baya                    # aidar | baya (default) | kseniya | xenia
SILERO_TTS_FORMAT=ogg                      # Recommended for Telegram
SILERO_TTS_SAMPLE_RATE=48000               # 8000 | 24000 | 48000 (default, best quality)
SILERO_TTS_TIMEOUT_SECS=60
```

Available speakers: `aidar`, `baya` (default), `kseniya`, `xenia`
Formats: `ogg` (recommended), `wav`
SSML support: set `ssml: true` for SSML markup with `<speak>`, `<break>`, `<prosody>` tags

### Media
- **Media Audio Provider** (`tool-media-audio`) - audio transcription
- **Media Image Provider** (`tool-media-image`) - image description
- **Media Video Provider** (`tool-media-video`) - video description
- **YT-DLP Provider** (`tool-ytdlp`) - video and audio download from various platforms

### Task Management
- **Todos Provider** (`tool-todos`) - task list management for planning
- **Delegation Provider** (`tool-delegation`) - async sub-agent spawn, wait, and cancellation
- **Reminder Provider** (`tool-reminder`) - reminder scheduling with pause/resume/retry

### Memory
- **Wiki Memory Provider** (`tool-wiki-memory`) - wiki memory list, read, delete
- **Compression Provider** (`tool-compression`) - message compression tools
- **Agents MD Provider** (`tool-agents-md`) - topic-scoped AGENTS.md editing

### File Handling
- **File Hoster Provider** (`tool-file-delivery`) - public file upload to temporary hosting (up to 4GB)

### Infrastructure
- **Manager Control Plane** (`manager-control-plane`) - topic CRUD, bindings, contexts, RBAC, audit trail
- **SSH MCP Provider** (`integration-ssh-mcp`) - SSH infrastructure with YOLO full-permission mode
- **Jira MCP Provider** (`integration-mcp-jira`) - Jira integration
- **Mattermost MCP Provider** (`integration-mcp-mattermost`) - Mattermost integration
- **Stack Logs Provider** (`tool-stack-logs`) - Docker Compose log access

</details>

<details>
<summary>Manager Control Plane</summary>

Programmatic topic management with RBAC, audit trail, and rollback support.

### Features
- **Topic CRUD:** `forum_topic_create`, `forum_topic_edit`, `forum_topic_close`, `forum_topic_reopen`, `forum_topic_delete`, `forum_topic_list`
- **Dynamic Bindings:** `topic_binding_set`, `topic_binding_get`, `topic_binding_delete`, `topic_binding_rollback`
- **Context Management:** `topic_context_upsert`, `topic_context_get`, `topic_context_delete`, `topic_context_rollback`
- **AGENTS.md Editing:** `topic_agents_md_get`, `topic_agents_md_update` (top-level agents only)
- **Infra Config:** `topic_infra_upsert`, `topic_infra_get`, `topic_infra_delete`, `topic_infra_probe`
- **Agent Profiles:** `agent_profile_upsert`, `agent_profile_get`, `agent_profile_delete`, `agent_profile_rollback`
- **Tools Management:** `topic_agent_tools_enable`, `topic_agent_tools_disable`, `topic_agent_tools_get`
- **Hooks Management:** `topic_agent_hooks_enable`, `topic_agent_hooks_disable`, `topic_agent_hooks_get`
- **Sandbox Management:** `topic_sandbox_list`, `topic_sandbox_destroy`, `topic_sandbox_recreate`

### RBAC Configuration
```dotenv
TELEGRAM_MANAGER_ALLOWED_USERS=123456789,987654321  # Users with manager control-plane access
MANAGER_HOME_CHAT_ID=-1001234567890        # Restrict to specific chat (optional)
MANAGER_HOME_THREAD_ID=1                   # Thread ID (optional)
MANAGER_HOME_AGENT_ID=control-plane       # Agent ID for manager home (optional)
```

**Note:** When `MANAGER_HOME_CHAT_ID` is set, manager control-plane tools are only available in the designated topic.

</details>

<details>
<summary>Security</summary>

### DM Tool Restrictions
SSH, Jira, and Mattermost tools are **blocked by default in private/DM chats** for security.

```dotenv
DM_ALLOWED_TOOLS=todos_write,todos_list,spawn_sub_agents,wait_sub_agents,cancel_sub_agents  # Allowlist mode
DM_BLOCKED_TOOLS=sandbox_exec  # Additional blocklist
```

### SSH Tool Access
SSH tools run in YOLO mode for configured topic infra bindings. Restrict access with topic-scoped `allowed_tool_modes`, DM tool restrictions, and secret refs.

</details>

## Wiki Memory

Persistent SQLx/Postgres-backed memory pages with deterministic storage and optional LLM-assisted extraction.

- **Storage:** Deterministic Markdown pages persisted as SQL rows keyed by `{prefix}/wiki/v1/contexts/{context_id}/pages/{slug}.md`
- **Background Planner:** Optionally uses LLM to extract structured memory after agent completion
- **Tools:** `wiki_memory_list`, `wiki_memory_read`, `wiki_memory_delete` (blocked for sub-agents)
- **Writer:** Enable with `WIKI_MEMORY_WRITER_ENABLED=true`; configure extraction model via `WIKI_MEMORY_WRITER_MODEL_ID` / `WIKI_MEMORY_WRITER_MODEL_PROVIDER`
- **Memory Hooks:** `episodic_extract` and `retrieval_advisor` activate when wiki memory writer is enabled

Details: `docs/wiki-memory.md`

## Agent Architecture

<details>
<summary>Internal Structure, Context, Hooks, Compaction</summary>

### Deterministic Context
- Topic-scoped `AGENTS.md`
- SQLx/Postgres-backed wiki memory
- Runtime context injections
- Enabled tools and profile instructions

### Loop Protection
Three-level loop detection system (`agent/loop_detection/`):
1. **Content Detector** - analyzes repeating agent messages
2. **Tool Detector** - tracks identical tool calls
3. **LLM Detector** - uses LLM to analyze loop patterns

**Configuration:** `LOOP_DETECTION_ENABLED`, `LOOP_TOOL_CALL_THRESHOLD` (5), `LOOP_LLM_CHECK_AFTER_TURNS` (30), `LOOP_SCOUT_MODEL`

### Runtime Compaction
Unified session-level compaction with a single path through `CompactionController`:
1. **Detect** - Pre-sampling budget check, context-limit retry, manual compact, or model-route downshift.
2. **Summarize** - Uses a normal configured LLM route as a provider-agnostic local summary backend (`LocalLlmSummary`).
3. **Replace Atomically** - Builds one `[OXIDE_COMPACTED_SUMMARY_V1]` handoff, preserves pinned state and safe recent tool context, validates tool-call integrity, and replaces hot memory in one step.

### Prompt Cache Optimization
Static prefix + dynamic suffix assembly maximizes provider-side prompt cache hit rate, with validated 80%+ cache hit on OpenCode Go.

**Architecture:**
- **Assembly order:** `[fallback + profile + workflow_guidance + structured_output]` (stable) + `[wiki_context]` + `[date_context]` (dynamic)
- **Tool schemas:** Compact sorted tool-name list in prompt text (2673->98 bytes, 27x reduction); full JSON schemas via native `tools[]` payload
- **Budget guard:** `compress` tool blocked at <85% context utilization to prevent premature compaction and cache reset
- **Cache telemetry:** `TokenUsage` includes `cached_tokens` and `cache_creation_tokens`, parsed for all 9 production providers

**Validated on OpenCode Go (`deepseek-v4-flash`):**
- Peak cache hit rate: **99.7%** after warmup (14 iterations, no compaction)
- Overall hit rate: **89.5%** across full task
- Estimated cost: **6.4x reduction** vs pre-optimization baseline ($0.014 vs $0.090 for same task)
- Premature compaction prevention (budget guard): cache hit preserved vs 93%->3.3% drop without guard

Cache telemetry parsers are deployed for all providers; live validation confirmed on OpenCode Go. Other providers return cache tokens when their upstream routes support it.

Details: `docs/tips/cache-hit.md`

### Hooks System
Extensible architecture for personalizing agent behavior:
- **Completion Hook** - task completion handling (protected, cannot be disabled)
- **Tool Access Policy** - blocks tools not allowed by current profile (protected, cannot be disabled)
- **Hot Context Health** - monitors context health during execution
- **Sub-Agent Safety** - ensures safe execution environments for delegated tasks
- **Search Budget** - limits search tool calls (10 per session)
- **Timeout Report** - provides detailed timeout reporting
- **Episodic Extract** - wiki memory extraction (active when writer enabled)
- **Retrieval Advisor** - wiki memory retrieval (active when writer enabled)

**Manageable Hooks:** `search_budget`, `timeout_report`, `retrieval_advisor`, `episodic_extract`

### Tool Providers
The agent uses a modular provider system, each offering a specialized set of tools. See [Tool Providers](#tool-providers) for the full list with configuration details.

</details>

## Reminder System

Enhanced reminder scheduling with pause/resume/retry support.

**Schedules:**
- `Once` - One-time reminder
- `Interval` - Recurring every N minutes/hours
- `Cron` - Complex schedules with timezone and weekday support

**Tools:**
- `reminder_schedule` - Create reminders with simplified args (`date`, `time`, `every_minutes`, `every_hours`, `timezone`, `weekdays`)
- `reminder_list` - List all reminders
- `reminder_cancel` - Cancel reminder
- `reminder_pause` / `reminder_resume` - Pause/resume with optional delay
- `reminder_retry` - Retry failed reminder

**Statuses:** `scheduled`, `paused`, `completed`, `cancelled`, `failed`

## Topic-Scoped Architecture

### Context Isolation
- Per-transport contexts live in `UserConfig.contexts` via `UserContextConfig`
- Context-scoped storage API: `save_agent_memory_for_context`, `load_agent_memory_for_context`, `clear_agent_memory_for_context`
- Chat history isolated via `scoped_chat_storage_id` format: `"{context_key}/{chat_uuid}"`

### Topic-Scoped Flows
- Flows attach/detach UX for per-session state management
- Stored by prefix: `users/{user_id}/topics/{context_key}/flows/{flow_id}/`
- `forum_topic_list` available for topic discovery (blocked for sub-agents)

### Topic-Scoped AGENTS.md
- Storage record: `TopicAgentsMdRecord`
- Orchestration via storage API and `prompt/composer.rs`
- Limited to 300 lines for `AGENTS.md`, 40 lines for `topic_context`
- Self-editing tools: `agents_md_get`, `agents_md_update` (top-level agents only)

## Deployment

See [`docs/deploy.md`](docs/deploy.md) for Docker, external services, sandbox, and operations notes.

## Usage

1.  Send `/start` to the bot.
2.  **Regular Mode:** Just write messages or send files/voice notes.
3.  **Agent Mode:** Click the "Agent Mode" button. Now the bot can execute code and use advanced tools.

<details>
<summary>Agent Command Examples and Control</summary>

**Agent Command Examples:**
- *"Write a python script that downloads the google homepage and finds the word 'Search' there"*
- *"Download video from YouTube via link [URL] and convert it to MP4 via FFmpeg"*
- *"Create a CSV file with weather data and upload it to file.io"*
- *"Find information about latest AI news via web search"*

**Topic Management Examples:**
- *"Create a new topic for Bug #1234"*
- *"Set agent profile 'developer' for this topic"*
- *"Enable Jira tools for this topic"*
- *"Get the audit trail for topic operations"*

**Control:** Use "Clear Context", "Change Model", "Exit Agent Mode" or topic-specific controls.
</details>

## Project Structure

<details>
<summary>File Tree (expand)</summary>

```text
crates/
├── oxide-agent-core/           # Domain logic, LLM integrations, hooks, compaction, storage
│   └── src/
│       ├── agent/              # Agent core and execution logic
│       │   ├── compaction/     # Compaction pipeline (12 modules)
│       │   ├── hooks/          # Execution hooks (7 hooks)
│       │   ├── loop_detection/ # Loop detection (content, tool, llm)
│       │   ├── providers/      # Tool providers
│       │   │   ├── ssh_mcp.rs            # SSH infrastructure
│       │   │   ├── jira_mcp/             # Jira integration
│       │   │   ├── mattermost_mcp/       # Mattermost integration
│       │   │   ├── brave_search/         # Brave Search API
│       │   │   ├── crw/                 # CRW search/scrape REST client
│       │   │   ├── tts/                  # Kokoro TTS
│       │   │   ├── silero_tts/           # Silero TTS
│       │   │   ├── manager_control_plane/ # Topic CRUD, RBAC
│       │   │   ├── wiki_memory.rs        # Wiki memory tools
│       │   │   └── ...
│       │   ├── tool_runtime/    # Typed tool registration and execution
│       │   ├── wiki_memory/     # Wiki memory planner, storage
│       │   ├── recovery/       # History repair, tool drift pruning
│       │   ├── runner/         # Execution loop, parallel tools
│       ├── llm/                # LLM provider integrations
│       │   ├── providers/      # Providers (chatgpt, zai, minimax, mistral, openrouter, opencode_go)
│       │   └── tool_correlation.rs
│       ├── sandbox/            # Sandbox facade and backends
│       │   ├── broker.rs        # Unix-socket sandbox broker protocol
│       │   ├── manager.rs      # Sandbox manager facade
│       │   └── traits.rs       # Sandbox backend traits
│       ├── storage/            # Storage facade, SQLx backend, control-plane records
│       ├── capabilities/       # Capability module manifests
│       └── config.rs
├── oxide-agent-runtime/        # Session orchestration, execution cycle, tool providers, sandbox
│   └── src/
│       └── agent/runtime/      # Progress runtime, transport-agnostic progress
├── oxide-agent-transport-telegram/  # Telegram transport layer
│   └── src/
│       ├── bot/agent_handlers/ # Agent lifecycle, controls, callbacks, reminders
│       ├── bot/views/agent.rs  # Agent Mode UI
│       ├── context.rs          # Context-scoped state
│       ├── topic_route.rs      # Topic binding resolution
│       ├── thread.rs           # Thread-aware session isolation
│       └── session_registry.rs
├── oxide-agent-transport-web/ # Web interface server + E2E test transport
│   └── src/
│       ├── server.rs           # HTTP API (axum), SSE, auth
│       ├── auth_helpers.rs     # Password auth, session management
│       ├── sse.rs              # Server-sent events
│       ├── static_assets.rs    # Frontend serving
│       ├── task_executor.rs    # Web task execution
│       ├── converters.rs       # API type conversion
│       └── providers.rs        # Scripted LLM provider
├── oxide-agent-web-contracts/ # Shared web API types
│   └── src/
│       ├── auth.rs             # Auth types
│       ├── config.rs           # Config types
│       ├── events.rs           # SSE event types
│       ├── sessions.rs         # Session types
│       └── tasks.rs            # Task types
├── oxide-agent-web-ui/        # Web interface frontend (Leptos SPA)
│   └── src/
│       ├── components/         # UI components
│       ├── routes/             # Page routes
│       ├── sse_client.rs       # SSE streaming client
│       └── styles/             # Dark theme, CSS
├── oxide-agent-sandboxd/       # Sandbox broker daemon
│   └── src/main.rs
└── oxide-agent-telegram-bot/   # Binary entry point and configuration
    └── src/
        ├── main.rs
        └── bin/chatgpt-login.rs  # ChatGPT OAuth login helper

tests/                          # Integration and functional tests
├── e2e/                        # E2E tests for web transport
│   ├── session_tests.rs
│   ├── sse_tests.rs
│   ├── compaction_regression_tests.rs
│   ├── delegation_tests.rs
│   ├── reminder_tests.rs
│   └── tool_latency_tests.rs
docs/                           # Documentation
├── wiki-memory.md              # Wiki memory system
├── silero-tts-api.md           # Silero TTS integration
├── stack-logs-stage0.md        # Stack logs tool
├── context-window-tracking.md  # Token budget management
├── tips/cache-hit.md           # Prompt cache optimization
├── hooks/                      # Hooks system documentation (9 files)
├── prd/                        # Product requirements documents
└── goals/                      # Development goal tracking
sandbox/                        # Docker configuration for sandbox
docker/                         # Docker profile overlays
config/                         # Configuration files (optional YAML)
.github/workflows/              # CI/CD workflows
```
</details>

## Feature Flags

### Profile Features

Each profile is a composition of atomic capability features. Build with `--no-default-features --features <PROFILE>`.

| Profile | Description | Key Components |
|---------|-------------|----------------|
| `profile-full` | Full production deployment | All features |
| `profile-embedded-opencode-local` | Telegram + local OpenCode | transport-telegram, storage-sqlx, llm-opencode-go, docker + sandboxd |
| `profile-web-embedded-opencode-local` | Web interface + local OpenCode | transport-web, storage-sqlx, llm-opencode-go, sandboxd |
| `profile-search-only` | Search-only agent | transport-telegram, web/tavily/brave-search/crw capability features |

### Atomic Features (selection)

| Category | Features |
|----------|----------|
| **LLM Providers** | `llm-chatgpt`, `llm-mistral`, `llm-minimax`, `llm-openai-base`, `llm-opencode-go`, `llm-openrouter` |
| **Search Tools** | `tool-tavily`, `tool-brave-search`, `tool-crw`, `tool-webfetch-md` |
| **Sandbox** | `tool-sandbox-exec`, `tool-sandbox-fileops`, `tool-sandbox-recreate` |
| **Sandbox Backends** | `sandbox-backend-docker-direct`, `sandbox-backend-sandboxd-client` |
| **Media** | `tool-media-audio`, `tool-media-image`, `tool-media-video`, `tool-ytdlp` |
| **TTS** | `tool-tts-kokoro`, `tool-tts-silero` |
| **Memory** | `tool-wiki-memory`, `tool-compression`, `tool-agents-md` |
| **Integrations** | `integration-mcp-jira`, `integration-mcp-mattermost`, `integration-ssh-mcp` |
| **Other** | `tool-todos`, `tool-delegation`, `tool-reminder`, `tool-file-delivery`, `tool-stack-logs`, `manager-control-plane` |

Build example:
```bash
cargo build --release --no-default-features --features profile-full
```

## Key Dependencies

<details>
<summary>Main Rust Libraries</summary>

- **teloxide** (0.17) - Telegram Bot API with macros and handlers
- **tokio** (1.52) - asynchronous runtime
- **sqlx-postgres/sqlx-core** (0.8) - PostgreSQL durable storage
- **bollard** (0.20) - Docker API for sandbox management
- **leptos** (0.8) - Web interface frontend (CSR)
- **axum** (0.7) - Web interface HTTP API
- **reqwest** (0.12/0.13) - HTTP client with multipart and streaming support
- **serde_json** (1.0) - JSON serialization/deserialization
- **tiktoken-rs** (0.9) - token counting for various models
- **claudius** (0.19) - MiniMax Anthropic SDK
- **rmcp** (1.2) - MCP client for Jira/SSH/Mattermost
- **moka** (0.12) - high-performance cache with TTL
- **chrono** (0.4) - date and time handling
- **thiserror** (2.0) - custom error creation
- **anyhow** (1.0) - simplified error handling in application

</details>

## License

The project is distributed under the **MIT License**. Details in the [LICENSE](https://github.com/0FL01/oxide-agent/blob/main/LICENSE) file.

Copyright (C) 2026 @0FL01
