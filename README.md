# Oxide Agent TG Bot

Universal Telegram bot with AI assistant, supporting multiple models, multimodality, and advanced **Agent Mode** with code execution.

## Description

<details>
<summary>ℹ️ About: Tech Stack (Rust 1.94), Integrations, and Architecture</summary>

This project is a Telegram bot that integrates with various Large Language Model (LLM) APIs to provide users with a multifunctional AI assistant. The bot can process text, voice, video messages, and images, work with documents, manage dialogue history, and perform complex tasks in an isolated sandbox.

The bot is developed using **Rust 1.94**, the `teloxide` library, and integrates with **6 Agent Mode LLM providers**: OpenCode Go, Zhipu AI/ZAI, MiniMax, Mistral, OpenRouter, and NVIDIA NIM.

### Architecture Highlights

- **Modular Workspace:** Separation into domain logic (core), orchestration (runtime), and transport layers
- **Transport-Agnostic Runtime:** Progress rendering and execution model can be adapted for Discord, Slack, etc.
- **Topic-Scoped Infrastructure:** Per-topic agent profiles, hooks, tools, and memory isolation
- **Manager Control Plane:** Programmatic topic management with RBAC, audit trail, and rollback support
- **Sandbox Backends:** Docker broker isolation by default, plus optional bare-host Bubblewrap mode
</details>

## Features

*   **🏗️ Workspace Architecture:** Modular crate design with clear separation of concerns:
    - `oxide-agent-core` - Domain logic, LLM integrations, hooks, compaction, storage
    - `oxide-agent-runtime` - Session orchestration, execution cycle, tool providers, sandbox
    - `oxide-agent-transport-telegram` - Telegram transport layer (teloxide integration)
    - `oxide-agent-transport-web` - E2E testing infrastructure with HTTP API
    - `oxide-agent-sandboxd` - Sandbox broker daemon for Docker access isolation in the default Compose deployment
    - `oxide-agent-telegram-bot` - Binary entry point and configuration

*   **🤖 Agent Mode:**
        <img width="974" height="747" alt="image_2026-01-11_20-58-21" src="https://github.com/user-attachments/assets/c99e55e4-8933-4ec8-9f50-22f7cbca4c77" />

    *   **Integrated Sandbox:** Safe execution of Python code and shell commands in isolated sandbox instances. Docker/broker is the default deployment path; Bubblewrap is available for bare-host setups.
    *   **Parallel Tool Execution:** Multiple tool calls in one LLM response execute concurrently for faster task completion.
    *   **Fire-and-Forget Checkpoint:** Memory persistence is async, non-blocking for reduced latency.
    *   **History Repair:** Validates tool_call_id before LLM calls; orphaned tool results prevented during compaction.
    *   **Cold-Start Tool Drift Pruning:** Removes stale tool calls from persisted memories on startup.
    *   **Tools:** Read/write files, execute commands, web search, work with video and file hosting.
    *   **📋 Task Management (Todos):** `write_todos` system for planning and tracking progress of complex requests.
    *   **Durable Context:** Topic `AGENTS.md`, wiki memory, runtime injections, and enabled tools provide deterministic prompt context.
    *   **📁 File Handling:** Accept files from user (up to 20MB), send to Telegram (up to 50MB), or upload to cloud (up to 4GB) with link generation.
    *   **🎬 Video Processing:** `yt-dlp` integration for downloading video and media files from the internet.
        <img width="977" height="762" alt="image" src="https://github.com/user-attachments/assets/1ffb66b7-559b-453f-9330-fbe27ccee90e" />

    *   **☁️ File Hosting:** Upload files from sandbox to public hosting with short retention time.
    *   **Web Search and Data Extraction:** DuckDuckGo and Tavily handle discovery; local `web_markdown` fetches one known URL as Markdown.
    *   **🔗 Hooks System:** Extensible architecture for intercepting and customizing agent behavior:
        - Completion Check Hook - validates task completion
        - Tool Access Policy - enforces profile-level tool allowlists and blocklists
        - Search Budget Hook - prevents infinite loops in tool calls
        - Soft Timeout Report Hook - provides detailed timeout reporting
        - Sub-Agent Safety - ensures safe execution environments
        - Registry - centralized hook management
    *   **⏱️ Universal Runtime:** Transport-agnostic progress rendering system that can be adapted for Discord, Slack, and other transports.
    *   **👥 Hierarchical Delegation:** The Main Agent spawns async Sub-Agents for parallel, independent subtasks. Each sub-agent runs in an isolated ephemeral session with a task-specific tool whitelist, inherits the topic AGENTS.md and parent cancellation, and returns results via background job tracking.
    *   **Autonomy:** Agent plans steps and selects tools itself.
    *   **Telegram Authorization:** Access control via `TELEGRAM_ALLOWED_USERS`.
    *   **Long-term Memory and Context:** Up to 200K tokens with automatic compression when limit reached.
    *   **Execution Progress:** Interactive display of current working step in Telegram.
*   **Multi-LLM Support:** 6 Agent Mode providers: OpenCode Go, Zhipu AI/ZAI, MiniMax, Mistral, OpenRouter, and NVIDIA NIM.
*   **Native Tool Calling:** Efficient use of tools in modern models with ToolCallCorrelation architecture.
*   **Multimedia Processing:**
    *   Voice and video messages (speech recognition via OpenRouter-hosted Gemini-family models or Voxtral).
    *   Images (analysis and description via multimodal models).
    *   Work with documents of various formats.
*   **🗣️ Voice Synthesis:** Kokoro TTS for English voice replies and Silero TTS for Russian voice replies.
*   **Context Management:** Dialogue history saved in Cloudflare R2 (S3) with context-scoped isolation per topic.
## System Requirements

<details>
<summary>🔑 API Keys and Infrastructure</summary>

### 🔑 API Keys (Mandatory)
| Provider | Variable | Description |
| :--- | :--- | :--- |
| **OpenCode Go** | `OPENCODE_GO_API_KEY` | **Primary Agent Mode provider** — recommended route: `deepseek-v4-flash` via `opencode-go`. [OpenCode](https://opencode.ai/) |
| **Telegram** | `TELEGRAM_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) |
| **Cloudflare R2** | `OXIDE_R2_*` | S3 storage (Access Key, Secret, Endpoint, Bucket) |
| **Zhipu AI (ZAI)** | `ZAI_API_KEY` | Required when using ZAI routes (`glm-4.7`, `glm-4.5-air`). [Zhipu AI](https://z.ai/) |
| **Mistral AI** | `MISTRAL_API_KEY` | Required for Mistral routes (`mistral-large-latest`, etc.) |

### 🤖 Supported LLM Providers for Agent Mode
The bot supports **6 main providers** for Agent Mode with tool calling:

*   **OpenCode Go** (`OPENCODE_GO_API_KEY`) — **primary (recommended) provider for Agent Mode**. Uses subscription OpenAI-compatible API at `opencode.ai/zen/go`. Recommended Agent Mode model: `deepseek-v4-flash` with provider `opencode-go`. Supports native tool calls (strict), structured JSON for DeepSeek V4 routes, adaptive throttling, unbounded retry, and reasoning content parsing.
*   **Zhipu AI / ZAI** (`ZAI_API_KEY`) — alternative provider for Agent Mode (`glm-4.7` or `glm-4.5-air`). Provides native tool-aware chat completions and reasoning.
*   **MiniMax** (`MINIMAX_API_KEY`) — Claude SDK-compatible provider via MiniMax API (`MiniMax-M2.7`).
*   **Mistral** (`MISTRAL_API_KEY`) — cost-effective agent routes and Voxtral audio transcription (`voxtral-mini-latest`).
*   **OpenRouter** (`OPENROUTER_API_KEY`) — multimodal/media routes and approved tool-capable Agent Mode routes, including Gemini-family model IDs through OpenRouter.

> [!NOTE]
> Voice recognition and image analysis require an explicit `MEDIA_MODEL_ID` / `MEDIA_MODEL_PROVIDER` route.

### 🛠 Infrastructure
*   **Docker** — run the default code sandbox (`agent-sandbox:latest`)
*   **Sandbox Broker** — Unix socket broker for Docker access isolation in Docker Compose (`SANDBOX_BACKEND=broker`)
*   **Bubblewrap** — optional bare-host sandbox backend without Docker daemon/socket access (`SANDBOX_BACKEND=bwrap`, see `docs/bwrap-sandbox.md`)
*   **Tavily API** — optional web search provider (`TAVILY_API_KEY`)
*   **DuckDuckGo** — built-in public web/news discovery provider (`DUCKDUCKGO_ENABLED`)
*   **Local Web Markdown** — lightweight single-URL HTTP fetch with HTML-to-Markdown conversion and response/output limits
*   **Browser Use Bridge** — self-hosted browser automation sidecar for high-level browser tasks (`BROWSER_USE_URL`), enabled by the web compose profile
*   **Kokoro TTS Server** — optional for English voice message synthesis (`KOKORO_TTS_URL`)
*   **Silero TTS Server** — optional for Russian voice message synthesis (`SILERO_TTS_URL`)
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

3.  **Build and run the bot:**
    ```bash
    docker-compose up --build -d
    ```

**Note:** The default Docker Compose configuration uses `SANDBOX_BACKEND=broker` which requires the `oxide-agent-sandboxd` container. To use direct Docker access, set `SANDBOX_BACKEND=docker`. For bare-host Bubblewrap mode, build `profile-host-bwrap` and follow `docs/bwrap-sandbox.md`.
</details>

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

   # Use a bucket-scoped token where possible.
   # For Cloudflare R2, Object Read & Write on the target bucket is sufficient.
   # Account-wide admin/list-all-buckets permissions are not required.

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
<summary>⚙️ Example Configuration File</summary>

```dotenv
# Telegram
TELEGRAM_TOKEN=YOUR_TOKEN
TELEGRAM_ALLOWED_USERS=ID1,ID2 # Telegram users allowed to use Agent Mode
REMINDER_AGENT_PROGRESS_ENABLED=false # Watch/ward reminders: hide progress spam
REMINDER_SILENT_NO_CHANGE_ENABLED=true # Watch/ward reminders: stay silent on no visible change

# Agent Configuration
AGENT_TIMEOUT_SECS=300          # Agent execution timeout
SEARCH_PROVIDER=tavily          # [DEPRECATED] use TAVILY_ENABLED / DUCKDUCKGO_ENABLED
DEBUG_MODE=false                # Debug logging mode

# Cloudflare R2 (S3)
OXIDE_R2_ACCESS_KEY_ID=...
OXIDE_R2_SECRET_ACCESS_KEY=...
OXIDE_R2_ENDPOINT_URL=...
OXIDE_R2_BUCKET_NAME=...
OXIDE_R2_REGION=auto           # S3-compatible storage region

# API Keys
MISTRAL_API_KEY=...
OPENROUTER_API_KEY=...
NVIDIA_API_KEY=...              # NVIDIA NIM / hosted integrate.api.nvidia.com
NVIDIA_API_BASE=https://integrate.api.nvidia.com/v1
ZAI_API_KEY=...                 # Zhipu AI / ZAI Provider
OPENCODE_GO_API_KEY=...         # OpenCode Go subscription provider
OPENCODE_GO_API_BASE=https://opencode.ai/zen/go/v1/chat/completions
MINIMAX_API_KEY=...             # MiniMax Provider (Claude SDK-compatible)
TAVILY_API_KEY=...             # Tavily web search in Agent mode (optional, enable via TAVILY_ENABLED=true)
DUCKDUCKGO_ENABLED=true        # DuckDuckGo web/news search (no API key or sidecar required)
DUCKDUCKGO_MIN_DELAY_MS=2500   # Process-wide DDG throttle
DUCKDUCKGO_JITTER_MS=1500      # Random DDG throttle jitter
# Browser Use self-hosted bridge (docker-compose.web.yml starts the local sidecar)
# BROWSER_USE_URL=http://127.0.0.1:8002
# BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE=3 # Optional retained reusable profiles per topic/context scope
# BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS=604800 # Optional idle/stale reusable profile TTL in the bridge
# BROWSER_USE_BRIDGE_BROWSER_READY_RETRIES=2 # Retry early transient browser readiness failures in the bridge
# BROWSER_USE_BRIDGE_BROWSER_READY_RETRY_DELAY_MS=750 # Delay between bridge readiness retries in milliseconds
# BROWSER_USE_MODEL_ID="mimo-v2.5" # Browser Use dedicated vision route
# BROWSER_USE_MODEL_PROVIDER="opencode-go" # Browser Use dedicated provider
```
</details>

For Browser Use task execution, Oxide sends the configured dedicated or inherited route to the bridge server-to-server. The web compose profile starts the local bridge and defaults the Browser Use route to OpenCode Go `mimo-v2.5`.

## Model Configuration

Set explicit agent and media routes through `.env`.

*   **Agent & Sub-agent (Recommended Models)**
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
Omitting the sub-agent block falls back to the agent model settings.

### Optional overrides
```dotenv
MEDIA_MODEL_ID="google/gemini-3.1-flash-lite-preview"
MEDIA_MODEL_PROVIDER="openrouter"
```

<details>
<summary>⚖️ Weighted Model Routes (Failover)</summary>

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
<summary>⚖️ Weighted failover with NVIDIA NIM</summary>

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
<summary>🌐 Browser Use default route</summary>

> **NOTE**: Browser Use is enabled by the web compose profile. Other profiles still require
> `tool-browser-use` at compile time and non-empty `BROWSER_USE_URL` at runtime.
> See `docs/browser-use.md`.

Browser Use can be pinned to a dedicated vision-capable route even when main/sub-agent stay on a different route. The web compose default is OpenCode Go `mimo-v2.5`; OpenCode Go `deepseek-v4-flash` is supported only for text-only browse tasks:

```dotenv
BROWSER_USE_MODEL_ID="mimo-v2.5"
BROWSER_USE_MODEL_PROVIDER="opencode-go"
```

Browser Use prefers this dedicated route over the currently active main/sub-agent route and falls back to the inherited route only when the dedicated override is absent.

</details>

<details>
<summary>🔄 Alternate provider example</summary>

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
| **OpenCode Go** | Primary (recommended) agent provider, subscription OpenAI-compatible API, DeepSeek V4 Flash, strict tool calling, structured output |
| **ZAI (Zhipu AI)** | Alternative agent provider, native tool-aware chat, GLM-4.7 / GLM-4.5-Air |
| **MiniMax** | Claude SDK-compatible, high context |
| **Mistral** | Generous free tier, includes Voxtral audio transcription |
| **OpenRouter** | Aggregator for various models, including Gemini-family model IDs |
| **NVIDIA NIM** | Tool calling support, hosted inference |

> **Note:** Gemini-family models are configured through OpenRouter routes, not a direct Google Gemini provider.

<details>
<summary>🔧 Tool Providers</summary>

### 🗣️ Kokoro TTS (Voice Synthesis)
Generates voice messages from agent output using local Kokoro TTS server.

**Tool:** `text_to_speech_en`

**Server Setup:** See [KOKORO-TTS-setup guide](https://github.com/0FL01/KOKORO-TTS-setup) for manual server setup.

**Configuration:**
```dotenv
KOKORO_TTS_URL=http://127.0.0.1:8000  # Default
KOKORO_TTS_VOICE=af_heart           # Default voice
KOKORO_TTS_FORMAT=ogg               # Recommended for Telegram
KOKORO_TTS_TIMEOUT_SECS=60
```

**Available Voices:** `af_bella`, `af_aoede`, `af_alloy`, `af_heart` (default)
**Formats:** `ogg` (recommended), `mp3`, `wav`

### 🇷🇺 Silero TTS (Russian Voice Synthesis)
Generates Russian voice messages from agent output using local Silero TTS server.

**Tool:** `text_to_speech_ru`

**Server Setup:** See [Oxide-Agent-TTS](https://github.com/0FL01/Oxide-Agent-TTS) for containerized Kokoro + Silero TTS servers with FastAPI.

**Configuration:**
```dotenv
SILERO_TTS_URL=http://127.0.0.1:8001       # Default
SILERO_TTS_SPEAKER=baya                    # aidar | baya (default) | kseniya | xenia
SILERO_TTS_FORMAT=ogg                      # Recommended for Telegram
SILERO_TTS_SAMPLE_RATE=48000               # 8000 | 24000 | 48000 (default, best quality)
SILERO_TTS_TIMEOUT_SECS=60
```

**Available Speakers:** `aidar`, `baya` (default), `kseniya`, `xenia`
**Formats:** `ogg` (recommended), `wav`
**SSML Support:** Set `ssml: true` for SSML markup with `<speak>`, `<break>`, `<prosody>` tags

**Migration Note:** Piper TTS has been replaced with Silero TTS. Use `text_to_speech_en` for Kokoro (English) and `text_to_speech_ru` for Silero (Russian).

### 🔐 SSH MCP Infrastructure
Topic-scoped SSH tools with approval flow for sensitive operations.

**Configuration:**
```dotenv
OXIDE_SSH_MCP_BINARY=/usr/local/bin/ssh-mcp
```

**Tools:** `ssh_exec`, `ssh_sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_check_process`, `ssh_send_file_to_user`

**Features:**
- Approval flow with TTL 600s
- Secret refs: `env:KEY` and `storage:PATH`
- Dangerous commands require approval (sudo, file edits on sensitive paths)

**Blocked in DM:** All SSH tools are blocked in private/DM chats by default.

</details>

<details>
<summary>🏗️ Manager Control Plane</summary>

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
<summary>🔒 Security</summary>

### DM Tool Restrictions
SSH, Jira, and Mattermost tools are **blocked by default in private/DM chats** for security.

**Configuration:**
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

## Agent Architecture


<details>
<summary>🏗 Internal Structure, Context, Hooks, Compaction</summary>

### Deterministic Context
- Topic-scoped `AGENTS.md`
- S3/R2-backed wiki memory
- Runtime context injections
- Enabled tools and profile instructions

### 🔄 Loop Protection
Three-level loop detection system (`agent/loop_detection/`):
1. **Content Detector** — analyzes repeating agent messages
2. **Tool Detector** — tracks identical tool calls
3. **LLM Detector** — uses LLM to analyze loop patterns

**Configuration:** `LOOP_DETECTION_ENABLED`, `LOOP_TOOL_CALL_THRESHOLD` (5), `LOOP_LLM_CHECK_AFTER_TURNS` (30), `LOOP_SCOUT_MODEL`

### 🔄 Runtime Compaction
Unified session-level compaction with a single path through `CompactionController`:
1. **Detect** — Pre-sampling budget check, context-limit retry, manual compact, or model-route downshift.
2. **Summarize** — Uses a normal configured LLM route as a provider-agnostic local summary backend (`LocalLlmSummary`).
3. **Replace Atomically** — Builds one `[OXIDE_COMPACTED_SUMMARY_V1]` handoff, preserves pinned state and safe recent tool context, validates tool-call integrity, and replaces hot memory in one step.

### 🔗 Hooks System
Extensible architecture for personalizing agent behavior:
- **Completion Hook** — task completion handling
- **Sub-Agent Safety** — ensures safe execution environments for delegated tasks
- **Search Budget** — limits search tool calls (10 per session)
- **Timeout Report** — provides detailed timeout reporting
- **Tool Access Policy** — blocks tools not allowed by current profile

**Protected Hooks (cannot be disabled):** `completion_check`, `tool_access_policy`

**Manageable Hooks:** `search_budget`, `timeout_report`, `retrieval_advisor`, `episodic_extract`

### 🛠️ Tool Providers
The agent uses a modular provider system, each offering a specialized set of tools:
- **Sandbox Provider** (`sandbox.rs`) — code execution, file read/write, shell commands
- **DuckDuckGo Provider** (`duckduckgo/`) — public web/news URL discovery
- **Tavily Provider** (`tavily.rs`) — web search and data extraction
- **WebFetch Markdown Provider** (`webfetch_md.rs`) — single-URL HTTP fetch with HTML-to-Markdown conversion and context-bomb limits
- **Todos Provider** (`todos.rs`) — task list management for long-term planning
- **YT-DLP Provider** (`ytdlp.rs`) — video and audio download from various platforms
- **File Hoster Provider** (`filehoster.rs`) — public file upload to temporary hosting (up to 4GB)
- **Delegation Provider** (`delegation.rs`) — async sub-agent spawn, wait, and cancellation for complex task decomposition
- **Reminder Provider** (`reminder.rs`) — reminder scheduling with pause/resume/retry
- **Kokoro TTS Provider** (`tts/`) — English voice message synthesis
- **Silero TTS Provider** (`silero_tts/`) — Russian voice message synthesis with SSML support
- **Jira MCP Provider** (`jira_mcp/`) — Jira integration
- **Mattermost MCP Provider** (`mattermost_mcp/`) — Mattermost integration
- **SSH MCP Provider** (`ssh_mcp.rs`) — SSH infrastructure with approval flow
- **Manager Control Plane** (`manager_control_plane/`) — Topic CRUD, RBAC, audit trail
- **Agents MD Provider** (`agents_md.rs`) — Topic-scoped AGENTS.md editing
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

<details>
<summary>🐳 Docker Architecture</summary>

### Services

The default Docker Compose deployment uses the broker backend. Bare-host Bubblewrap mode is documented separately in `docs/bwrap-sandbox.md` and is not enabled by this Compose file.

1. **sandbox_image**
    - Builds the selected sandbox image variant, with full/dev using `sandbox/Dockerfile.dev`
    - One-shot build service used during `docker compose up --build`

2. **oxide_agent** (main bot)
   - Builds from `docker/Dockerfile.app` with the full profile by default
   - Network mode: `host`
   - Mounts: `./config:/app/config`, `sandboxd-run:/run/sandboxd`
   - Environment: `SANDBOX_BACKEND=broker`, `SANDBOXD_SOCKET=/run/sandboxd/sandboxd.sock`

3. **sandboxd** (broker daemon)
   - Uses the same full-profile `docker/Dockerfile.app` image
   - Command: `./oxide-agent-sandboxd`
   - Runs as user 0 (privileged for Docker access)
   - Mounts: `/var/run/docker.sock:/var/run/docker.sock` (only sandboxd has Docker access)
   - Socket: `/run/sandboxd/sandboxd.sock`

### Sandbox Broker Protocol
- Unix socket communication with binary serialization (bincode)
- Frame format: `[u64 length][payload]`
- Operations: List, Inspect, Create, Delete, Exec, Read/Write files, Upload/Download, Cleanup

### CI/CD Pipeline
- **Test job:** `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt`
- **Validate credentials:** Integration tests with real API keys
- **Deploy job:** SSH deployment to production server, dynamic docker-compose generation
- **Docker workflow:** Multi-platform builds with Docker Buildx, pushes to Docker Hub
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

**Topic Management Examples:**
- *"Create a new topic for Bug #1234"*
- *"Set agent profile 'developer' for this topic"*
- *"Enable Jira tools for this topic"*
- *"Get the audit trail for topic operations"*

**Control:** Use "Clear Context", "Change Model", "Exit Agent Mode" or topic-specific controls.
</details>

## Project Structure

<details>
<summary>📂 File Tree (expand)</summary>

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
│       │   │   ├── tts/                  # Kokoro TTS
│       │   │   ├── silero_tts/           # Silero TTS
│       │   │   ├── manager_control_plane/ # Topic CRUD, RBAC
│       │   │   └── ...
│       │   ├── recovery/       # History repair, tool drift pruning
│       │   ├── runner/         # Execution loop, parallel tools
│       ├── llm/                # LLM provider integrations
│       │   ├── providers/      # Providers (zai, minimax, mistral, openrouter, ...)
│       │   └── tool_correlation.rs
│       ├── storage/            # Storage facade, R2 backend, control-plane records
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
├── oxide-agent-transport-web/ # E2E test transport with HTTP API
│   └── src/
│       ├── server.rs           # HTTP API (axum)
│       ├── providers.rs        # Scripted LLM provider
│       └── storage.rs          # In-memory storage
├── oxide-agent-sandboxd/       # Sandbox broker daemon
│   └── src/main.rs
└── oxide-agent-telegram-bot/   # Binary entry point and configuration
    └── src/main.rs

tests/                          # Integration and functional tests
├── e2e/                        # E2E tests for web transport
│   ├── session_tests.rs
│   ├── sse_tests.rs
│   ├── compaction_regression_tests.rs
│   ├── delegation_tests.rs
│   ├── reminder_tests.rs
│   └── tool_latency_tests.rs
docs/                           # Documentation
├── index.md                    # Main documentation hub
└── hooks/                      # Hooks system documentation (11 files)
sandbox/                        # Docker configuration for sandbox
config/                         # Configuration files (default.yaml, local.yaml, etc.)
.github/workflows/              # CI/CD workflows (ci-cd.yml, docker.yml)
```
</details>

## Key Dependencies

<details>
<summary>📦 Main Rust Libraries</summary>

**Main libraries:**

- **teloxide** (0.17.0) — Telegram Bot API with macros and handlers
- **tokio** (1.48) — asynchronous runtime
- **async-openai** (0.33.1) — work with OpenAI-compatible APIs (updated)
- **aws-sdk-s3** (1.119.0) — Cloudflare R2 integration
- **bollard** (0.20.2) — Docker API for sandbox management (updated)
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

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `tavily` | Enable Tavily web search provider | Enabled |
| `tool-duckduckgo` | Enable DuckDuckGo web/news search provider | Enabled |
| `jira` | Enable Jira MCP integration | Disabled |
| `mattermost` | Enable Mattermost MCP integration | Disabled |

Build with features:
```bash
cargo build --release --no-default-features --features profile-full
```

## License

The project is distributed under the **GNU Affero General Public License v3 (AGPL-3.0)**. Details in the [LICENSE](https://github.com/0FL01/oxide-agent/blob/main/LICENSE) file.

Copyright (C) 2026 @0FL01
