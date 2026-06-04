# Oxide Agent

Universal Telegram bot with AI assistant, supporting multiple models, multimodality, and advanced **Agent Mode** with code execution, plus a **Web Interface** for browser-based chat with the agent.

## Description

<details>
<summary>About: Tech Stack (Rust 1.94), Integrations, and Architecture</summary>

This project is a Telegram bot that integrates with various Large Language Model (LLM) APIs to provide users with a multifunctional AI assistant. The bot can process text, voice, video messages, and images, work with documents, manage dialogue history, and perform complex tasks in an isolated sandbox.

The bot is developed using **Rust 1.94**, the `teloxide` library, and integrates with **8 Agent Mode LLM providers**: ChatGPT/Codex (OAuth), OpenCode Go, OpenCode Zen, Zhipu AI/ZAI, MiniMax, Mistral, OpenRouter, and NVIDIA NIM.

### Architecture Highlights

- **Modular Workspace:** Separation into domain logic (core), orchestration (runtime), and transport layers
- **Transport-Agnostic Runtime:** Progress rendering and execution model can be adapted for Discord, Slack, etc.
- **Web Interface:** Browser-based chat with the agent (Leptos SPA, SSE streaming, dark theme, markdown rendering)
- **Topic-Scoped Infrastructure:** Per-topic agent profiles, hooks, tools, and memory isolation
- **Manager Control Plane:** Programmatic topic management with RBAC, audit trail, and rollback support
- **Sandbox Backends:** Docker broker isolation by default, plus optional bare-host Bubblewrap mode
- **Wiki Memory:** S3/R2-backed persistent memory with optional LLM-assisted extraction
- **Prompt Cache Optimization:** Static prefix + dynamic suffix assembly for maximum cache hit rate across all providers
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
        <img width="974" height="747" alt="image_2026-01-11_20-58-21" src="https://github.com/user-attachments/assets/c99e55e4-8933-4ec8-9f50-22f7cbca4c77" />

    *   **Integrated Sandbox:** Safe execution of Python code and shell commands in isolated sandbox instances. Docker/broker is the default deployment path; Bubblewrap is available for bare-host setups.
    *   **Parallel Tool Execution:** Multiple tool calls in one LLM response execute concurrently for faster task completion.
    *   **Fire-and-Forget Checkpoint:** Memory persistence is async, non-blocking for reduced latency.
    *   **History Repair:** Validates tool_call_id before LLM calls; orphaned tool results prevented during compaction.
    *   **Tools:** Read/write files, execute commands, web search, work with video and file hosting.
    *   **Task Management (Todos):** `write_todos` system for planning and tracking progress of complex requests.
    *   **Durable Context:** Topic `AGENTS.md`, wiki memory, runtime injections, and enabled tools provide deterministic prompt context.
    *   **File Handling:** Accept files from user (up to 20MB), send to Telegram (up to 50MB), or upload to cloud (up to 4GB) with link generation.
    *   **Video Processing:** `yt-dlp` integration for downloading video and media files from the internet.
        <img width="977" height="762" alt="image" src="https://github.com/user-attachments/assets/1ffb66b7-559b-453f-9230-fbe27ccee90e" />

    *   **File Hosting:** Upload files from sandbox to public hosting with short retention time.
    *   **Web Search and Data Extraction:** DuckDuckGo, Tavily, SearXNG, and Crawl4AI handle discovery and extraction; local `web_markdown` fetches one known URL as Markdown.
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
*   **Multi-LLM Support:** 8 Agent Mode providers: ChatGPT/Codex (OAuth), OpenCode Go, OpenCode Zen, Zhipu AI/ZAI, MiniMax, Mistral, OpenRouter, and NVIDIA NIM.
*   **Native Tool Calling:** Efficient use of tools in modern models with ToolCallCorrelation architecture.
*   **Web Interface:** Browser-based chat with the agent -- Leptos SPA with SSE streaming for real-time responses, dark theme, and markdown rendering.
*   **Multimedia Processing:**
    *   Voice and video messages (speech recognition via OpenRouter-hosted Gemini-family models or Voxtral).
    *   Images (analysis and description via multimodal models).
    *   Work with documents of various formats.
*   **Voice Synthesis:** Kokoro TTS for English voice replies and Silero TTS for Russian voice replies.
*   **Context Management:** Dialogue history saved in Cloudflare R2 (S3) with context-scoped isolation per topic.
*   **Wiki Memory:** Persistent S3/R2-backed memory pages with optional LLM-assisted extraction and retrieval.
*   **Prompt Cache Optimization:** Static prefix + dynamic suffix assembly order maximizes cache hit rate across all providers.

## System Requirements

<details>
<summary>API Keys and Infrastructure</summary>

### API Keys (Mandatory)
| Provider | Variable | Description |
| :--- | :--- | :--- |
| **OpenCode Go** | `OPENCODE_GO_API_KEY` | **Primary Agent Mode provider** - recommended route: `deepseek-v4-flash` via `opencode-go`. [OpenCode](https://opencode.ai/) |
| **Telegram** | `TELEGRAM_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) |
| **Cloudflare R2** | `OXIDE_R2_*` | S3 storage (Access Key, Secret, Endpoint, Bucket) |
| **Zhipu AI (ZAI)** | `ZAI_API_KEY` | Required when using ZAI routes (`glm-4.7`, `glm-4.5-air`). [Zhipu AI](https://z.ai/) |
| **Mistral AI** | `MISTRAL_API_KEY` | Required for Mistral routes (`mistral-large-latest`, etc.) |

### Supported LLM Providers for Agent Mode
The bot supports **8 providers** for Agent Mode with tool calling:

*   **OpenCode Go** (`OPENCODE_GO_API_KEY`) - **primary (recommended) provider for Agent Mode**. Uses subscription OpenAI-compatible API at `opencode.ai/zen/go`. Recommended Agent Mode model: `deepseek-v4-flash` with provider `opencode-go`. Supports native tool calls (strict), structured JSON for DeepSeek V4 routes, reasoning content parsing, adaptive throttling, and unbounded retry.
*   **OpenCode Zen** - Free-tier filtered variant of OpenCode Go. Same provider code, filtered to free-only models via discovery. Provider alias: `opencode-zen`.
*   **ChatGPT/Codex** (`CHATGPT_AUTH_PATH`) - Headless OAuth provider for OpenAI Codex Responses API at `chatgpt.com/backend-api/codex/responses`. SSE streaming. No audio/image support. Use `cargo run -p oxide-agent-telegram-bot --bin chatgpt-login -- login` for initial auth.
*   **Zhipu AI / ZAI** (`ZAI_API_KEY`) - Alternative provider for Agent Mode (`glm-4.7` or `glm-4.5-air`). Provides native tool-aware chat completions and reasoning.
*   **MiniMax** (`MINIMAX_API_KEY`) - Claude SDK-compatible provider via MiniMax API (`MiniMax-M2.7`).
*   **Mistral** (`MISTRAL_API_KEY`) - Cost-effective agent routes and Voxtral audio transcription (`voxtral-mini-latest`).
*   **OpenRouter** (`OPENROUTER_API_KEY`) - Multimodal/media routes and approved tool-capable Agent Mode routes, including Gemini-family model IDs through OpenRouter.
*   **NVIDIA NIM** (`NVIDIA_API_KEY`) - Tool calling support for approved model routes, hosted inference.

> [!NOTE]
> Voice recognition and image analysis require an explicit `MEDIA_MODEL_ID` / `MEDIA_MODEL_PROVIDER` route.

### Infrastructure
*   **Docker** - run the default code sandbox (`agent-sandbox:latest`)
*   **Sandbox Broker** - Unix socket broker for Docker access isolation in Docker Compose (`SANDBOX_BACKEND=broker`)
*   **Bubblewrap** - optional bare-host sandbox backend without Docker daemon/socket access (`SANDBOX_BACKEND=bwrap`, see `docs/bwrap-sandbox.md`)
*   **Tavily API** - optional web search provider (`TAVILY_API_KEY`)
*   **DuckDuckGo** - built-in public web/news discovery provider (`DUCKDUCKGO_ENABLED`)
*   **SearXNG** - optional self-hosted search aggregator (`SEARXNG_ENABLED`)
*   **Crawl4AI** - optional browser-rendered Markdown extraction (`OXIDE_CRAWL4AI_BASE_URL`)
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

<details>
<summary>Alpine 3.23 deployment from the release binary (embedded profile)</summary>

This path is for the prebuilt `x86_64` release artifact built with the embedded profile. Download the Alpine release archive from GitHub Releases, unpack it under `/opt/oxide-agent`, and run the binary directly or through OpenRC.

1. Install host packages:

   ```bash
   apk add --no-cache bubblewrap ca-certificates tar xz curl
   ```

2. Create a dedicated service user and prepare directories:

   ```bash
   addgroup -S oxide
   adduser -S -D -H -h /var/lib/oxide-agent -s /sbin/nologin -G oxide oxide
   mkdir -p /opt/oxide-agent/bin
   mkdir -p /opt/oxide-agent/bwrap-images
   mkdir -p /var/lib/oxide-agent/sandbox/scopes
   mkdir -p /var/lib/oxide-agent/sandbox/locks
   mkdir -p /var/lib/oxide-agent/sandbox/root-upper
   mkdir -p /var/log/oxide-agent
   chown -R oxide:oxide /opt/oxide-agent /var/lib/oxide-agent /var/log/oxide-agent
   ```

3. Download the Alpine release archive from GitHub Releases, unpack it, and move the binary to `/opt/oxide-agent/bin/oxide-agent-telegram-bot`.

4. Create `/opt/oxide-agent/.env`. The release binary reads `.env` from its working directory. Minimal example:

   ```dotenv
   TELEGRAM_TOKEN=YOUR_TELEGRAM_BOT_TOKEN
   TELEGRAM_ALLOWED_USERS=123456789
   TELEGRAM_MANAGER_ALLOWED_USERS=123456789

   OXIDE_R2_ACCESS_KEY_ID=YOUR_R2_KEY
   OXIDE_R2_SECRET_ACCESS_KEY=YOUR_R2_SECRET
   OXIDE_R2_ENDPOINT_URL=https://<account_id>.r2.cloudflarestorage.com
   OXIDE_R2_BUCKET_NAME=your_bucket
   OXIDE_R2_REGION=auto

   OPENCODE_GO_API_KEY=YOUR_OPENCODE_GO_API_KEY
   OPENCODE_GO_API_BASE=https://opencode.ai/zen/go/v1/chat/completions

   AGENT_MODEL_ID=deepseek-v4-flash
   AGENT_MODEL_PROVIDER=opencode-go
   SUB_AGENT_MODEL_ID=deepseek-v4-flash
   SUB_AGENT_MODEL_PROVIDER=opencode-go

   SANDBOX_BACKEND=bwrap
   BWRAP_BIN=/usr/bin/bwrap
   BWRAP_IMAGE=alpine-3.23-dev
   BWRAP_IMAGE_BOOTSTRAP=download
   BWRAP_IMAGE_URL=https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/x86_64/alpine-minirootfs-3.23.4-x86_64.tar.gz
   BWRAP_IMAGE_SHA256=85498865362aa7ebececa0d725a2f2e4db7ac4e4b2850b8df21645afa0d03ee3
   BWRAP_IMAGE_PACKAGE_MANAGER=apk
   BWRAP_IMAGE_STORE=/opt/oxide-agent/bwrap-images
   BWRAP_STATE_DIR=/var/lib/oxide-agent/sandbox/scopes
   BWRAP_LOCK_DIR=/var/lib/oxide-agent/sandbox/locks
   BWRAP_ROOT_MODE=overlay-rw
   BWRAP_ROOT_UPPER_DIR=/var/lib/oxide-agent/sandbox/root-upper
   BWRAP_NET=host
   BWRAP_ALLOW_OVERLAY=true
   BWRAP_RESOLV_CONF=auto
   BWRAP_DISABLE_NESTED_USERNS=true

   RUST_LOG=oxide_agent_core=info,oxide_agent_transport_telegram=info,oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn
   DEBUG_MODE=false
   ```

   When `SANDBOX_BACKEND=bwrap` is set, startup checks `/usr/bin/bwrap` immediately. If bubblewrap is missing, the bot exits with an error telling you to install `bubblewrap`, set `BWRAP_BIN`, or choose another sandbox backend.

   `BWRAP_RESOLV_CONF=auto` stages the host resolver config and bind-mounts it into the sandbox. With `BWRAP_ROOT_MODE=overlay-rw`, the bot also creates the missing `/etc/resolv.conf` bind target for Alpine minirootfs images.

5. On startup, `BWRAP_IMAGE_BOOTSTRAP=download` downloads the Alpine minirootfs, verifies `BWRAP_IMAGE_SHA256`, extracts it to `/opt/oxide-agent/bwrap-images/alpine-3.23-dev`, and writes the bwrap `image.json`. If `/opt/oxide-agent/bwrap-images/alpine-3.23-dev/image.json` already exists, bootstrap is a no-op.

6. Optional OpenRC wrapper:

   ```bash
   cat >/opt/oxide-agent/bin/run-oxide-agent.sh <<'EOF'
   #!/bin/sh
   set -a
   . /opt/oxide-agent/.env
   set +a
   cd /opt/oxide-agent
   exec /opt/oxide-agent/bin/oxide-agent-telegram-bot
   EOF
   chmod +x /opt/oxide-agent/bin/run-oxide-agent.sh
   chown oxide:oxide /opt/oxide-agent/bin/run-oxide-agent.sh
   ```

7. Optional OpenRC service:

   ```bash
   cat >/etc/init.d/oxide-agent <<'EOF'
   #!/sbin/openrc-run

   name="oxide-agent"
   description="Oxide Agent Telegram bot"
   command="/opt/oxide-agent/bin/run-oxide-agent.sh"
   directory="/opt/oxide-agent"
   command_user="oxide:oxide"
   command_background="true"
   pidfile="/run/${RC_SVCNAME}.pid"
   output_log="/var/log/oxide-agent/current.log"
   error_log="/var/log/oxide-agent/current.log"

   depend() {
       need net
       use dns logger
   }
   EOF
   chmod +x /etc/init.d/oxide-agent
   rc-update add oxide-agent default
   rc-service oxide-agent start
   ```

   The bot writes structured logs to `stderr`, so the OpenRC example sends both `stdout` and `stderr` to the same `current.log` file.

8. Manual verification:

   ```bash
   cd /opt/oxide-agent
   ./bin/oxide-agent-telegram-bot capabilities --compiled --json
   tail -f /var/log/oxide-agent/current.log
   ```

For a more detailed bare-host Bubblewrap reference, see `docs/bwrap-sandbox.md`.
</details>

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

# Cloudflare R2 (S3)
OXIDE_R2_ACCESS_KEY_ID=...
OXIDE_R2_SECRET_ACCESS_KEY=...
OXIDE_R2_ENDPOINT_URL=...
OXIDE_R2_BUCKET_NAME=...
OXIDE_R2_REGION=auto

# API Keys
CHATGPT_AUTH_PATH=/app/config/chatgpt/auth.json
MISTRAL_API_KEY=...
OPENROUTER_API_KEY=...
NVIDIA_API_KEY=...
NVIDIA_API_BASE=https://integrate.api.nvidia.com/v1
ZAI_API_KEY=...
OPENCODE_GO_API_KEY=...
OPENCODE_GO_API_BASE=https://opencode.ai/zen/go/v1/chat/completions
MINIMAX_API_KEY=...

# Web Search Providers (can be enabled together)
TAVILY_API_KEY=...
# BRAVE_SEARCH_API_KEY=...
# BRAVE_SEARCH_ENABLED=true
# Brave Search — primary indexed web discovery when BRAVE_SEARCH_API_KEY is configured.
DUCKDUCKGO_ENABLED=true
DUCKDUCKGO_MIN_DELAY_MS=2500
DUCKDUCKGO_JITTER_MS=1500
# SEARXNG_ENABLED=true
# SEARXNG_URL=http://127.0.0.1:8081
# SEARXNG_BEARER_TOKEN=...
# SearXNG — fallback/self-hosted aggregator.

# Crawl4AI (browser-rendered Markdown)
# OXIDE_CRAWL4AI_BASE_URL=http://127.0.0.1:11235
# OXIDE_CRAWL4AI_API_TOKEN=...
# Crawl4AI — browser-rendered opener for selected URLs.

# Wiki Memory Writer (background, optional LLM-assisted)
# WIKI_MEMORY_WRITER_ENABLED=true
# WIKI_MEMORY_WRITER_MODEL_ID="google/gemini-3-flash-preview"
# WIKI_MEMORY_WRITER_MODEL_PROVIDER="openrouter"
```
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

  **Alternative (ZAI):** If you prefer the ZAI provider, use **glm-4.7** for the Main Agent and **glm-4.5-air** for the Sub-Agent:
```dotenv
AGENT_MODEL_ID="glm-4.7"
AGENT_MODEL_PROVIDER="zai"

SUB_AGENT_MODEL_ID="glm-4.5-air"
SUB_AGENT_MODEL_PROVIDER="zai"
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
# Priority: OpenCode Go (DeepSeek V4 Flash) > ZAI (GLM-4.7) > Mistral
AGENT_MODEL_ROUTES__0__ID="deepseek-v4-flash"
AGENT_MODEL_ROUTES__0__PROVIDER="opencode-go"
AGENT_MODEL_ROUTES__0__WEIGHT=10

AGENT_MODEL_ROUTES__1__ID="glm-4.7"
AGENT_MODEL_ROUTES__1__PROVIDER="zai"
AGENT_MODEL_ROUTES__1__WEIGHT=5

AGENT_MODEL_ROUTES__2__ID="mistral-small-2603"
AGENT_MODEL_ROUTES__2__PROVIDER="mistral"
AGENT_MODEL_ROUTES__2__WEIGHT=2
```

</details>

<details>
<summary>Weighted failover with NVIDIA NIM</summary>

Use NVIDIA NIM only with the explicit Agent Mode allowlist. If you are unsure, keep NIM behind a proven primary route first:

```dotenv
NVIDIA_API_KEY=...
NVIDIA_API_BASE="https://integrate.api.nvidia.com/v1"

AGENT_MODEL_ROUTES__0__ID="deepseek-v4-flash"
AGENT_MODEL_ROUTES__0__PROVIDER="opencode-go"
AGENT_MODEL_ROUTES__0__WEIGHT=5

AGENT_MODEL_ROUTES__1__ID="deepseek-ai/deepseek-v4-flash"
AGENT_MODEL_ROUTES__1__PROVIDER="nvidia"
AGENT_MODEL_ROUTES__1__WEIGHT=3
```

The agent runtime skips unsupported NVIDIA NIM routes before tool-enabled execution. Structured output is enabled only for explicitly approved model routes.

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
| **NVIDIA NIM** | Tool calling support, hosted inference |

> **Note:** Gemini-family models are configured through OpenRouter routes, not a direct Google Gemini provider.

<details>
<summary>Tool Providers</summary>

### Web Search and Extraction
- **DuckDuckGo Provider** (`tool-duckduckgo`) - public web/news URL discovery, no API key required
- **Brave Search Provider** (`tool-brave-search`) - primary indexed web discovery when `BRAVE_SEARCH_API_KEY` is configured
- **Tavily Provider** (`tool-tavily`) - web search and data extraction
- **SearXNG Provider** (`tool-searxng`) - fallback/self-hosted aggregator
- **Crawl4AI Provider** (`tool-crawl4ai-markdown`) - browser-rendered opener for selected search result URLs
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
- **SSH MCP Provider** (`integration-ssh-mcp`) - SSH infrastructure with approval flow
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

### SSH Approval Flow
Sensitive SSH operations require operator approval with single-use tokens.

**Approval Required Modes:**
- `SudoExec` - Remote commands with sudo
- `ApplyFileEdit` - In-place file edits
- Dangerous commands: `rm -rf`, `shutdown`, `reboot`, `systemctl`, `docker compose down`, `kubectl`, `terraform apply/destroy`
- Sensitive paths: `/etc/`, `/root/`, `/home/`, `.ssh`, `systemd`, `nginx`, `postgresql`

**Approval Flow:**
1. Agent requests SSH action
2. If approval required, returns approval request ID
3. Operator grants approval via `grant_ssh_approval(request_id)`
4. Agent retries with approval token
5. Token consumed (single-use), TTL 600s

</details>

## Wiki Memory

Persistent S3/R2-backed memory pages with deterministic storage and optional LLM-assisted extraction.

- **Storage:** Deterministic Markdown pages at `{prefix}/wiki/v1/contexts/{context_id}/pages/{slug}.md` in S3/R2
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
- S3/R2-backed wiki memory
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
Static prefix + dynamic suffix assembly maximizes provider-side prompt cache hit rate.

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
│       │   │   ├── duckduckgo/           # DuckDuckGo search
│       │   │   ├── brave_search/         # Brave Search API
│       │   │   ├── searxng/             # SearXNG search
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
│       │   ├── providers/      # Providers (chatgpt, zai, minimax, mistral, openrouter, nvidia, opencode_go)
│       │   └── tool_correlation.rs
│       ├── sandbox/            # Sandbox facade and backends
│       │   ├── bwrap/          # Bubblewrap backend (14 modules)
│       │   ├── manager.rs      # Sandbox manager facade
│       │   ├── broker.rs       # Broker client/protocol
│       │   └── traits.rs       # Sandbox backend traits
│       ├── storage/            # Storage facade, R2 backend, control-plane records
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
├── bwrap-sandbox.md            # Bubblewrap sandbox backend
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
| `profile-embedded-opencode-local` | Telegram + local OpenCode, bwrap | transport-telegram, storage-s3-r2, llm-opencode-go, bwrap |
| `profile-web-embedded-opencode-local` | Web interface + local OpenCode | transport-web, storage-s3-r2, llm-opencode-go, bwrap |
| `profile-lite` | Minimal Telegram bot | transport-telegram, storage-s3-r2, llm-opencode-go, todos, webfetch, reminders |
| `profile-search-only` | Search-only agent | transport-telegram, web/tavily/duckduckgo/brave-search/searxng capability features |
| `profile-no-sandbox` | Telegram without sandbox | transport-telegram, storage-s3-r2, llm-opencode-go, wiki memory |
| `profile-media-enabled` | Media processing only | transport-telegram, media audio/image/video, file delivery |
| `profile-host-bwrap` | Host-level bwrap, no Docker | transport-telegram, llm-opencode-go + openrouter, bwrap |

### Atomic Features (selection)

| Category | Features |
|----------|----------|
| **LLM Providers** | `llm-chatgpt`, `llm-mistral`, `llm-minimax`, `llm-zai`, `llm-nvidia`, `llm-opencode-go`, `llm-openrouter` |
| **Search Tools** | `tool-tavily`, `tool-duckduckgo`, `tool-brave-search`, `tool-searxng`, `tool-crawl4ai-markdown`, `tool-webfetch-md` |
| **Sandbox** | `tool-sandbox-exec`, `tool-sandbox-fileops`, `tool-sandbox-recreate` |
| **Sandbox Backends** | `sandbox-backend-docker-direct`, `sandbox-backend-sandboxd-client`, `sandbox-backend-bwrap` |
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
- **async-openai** (0.40) - OpenAI-compatible APIs
- **aws-sdk-s3** (1.127) - Cloudflare R2 integration
- **bollard** (0.20) - Docker API for sandbox management
- **leptos** (0.8) - Web interface frontend (CSR)
- **axum** (0.7) - Web interface HTTP API
- **reqwest** (0.12/0.13) - HTTP client with multipart and streaming support
- **serde_json** (1.0) - JSON serialization/deserialization
- **tiktoken-rs** (0.9) - token counting for various models
- **claudius** (0.19) - MiniMax Anthropic SDK
- **zai-rs** (0.1) - Zhipu AI SDK
- **rmcp** (1.2) - MCP client for Jira/SSH/Mattermost
- **moka** (0.12) - high-performance cache with TTL
- **chrono** (0.4) - date and time handling
- **thiserror** (2.0) - custom error creation
- **anyhow** (1.0) - simplified error handling in application

</details>

## License

The project is distributed under the **MIT License**. Details in the [LICENSE](https://github.com/0FL01/oxide-agent/blob/main/LICENSE) file.

Copyright (C) 2026 @0FL01
