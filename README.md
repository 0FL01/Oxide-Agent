# Oxide Agent TG Bot

[(Russian README)](README-ru.md)

Universal Telegram bot with AI assistant, supporting multiple models, multimodality, and advanced **Agent Mode** with code execution.

## Description

<details>
<summary>ℹ️ About: Tech Stack (Rust 1.94), Integrations, and Architecture</summary>

This project is a Telegram bot that integrates with various Large Language Model (LLM) APIs to provide users with a multifunctional AI assistant. The bot can process text, voice, video messages, and images, work with documents, manage dialogue history, and perform complex tasks in an isolated sandbox.

The bot is developed using **Rust 1.94**, the `teloxide` library, and integrates with **5 main AI providers** for Chat/Agent mode (Zhipu AI/ZAI, MiniMax, OpenRouter, Mistral, Google Gemini), along with Groq support.

### Architecture Highlights

- **Modular Workspace:** Separation into domain logic (core), orchestration (runtime), and transport layers
- **Transport-Agnostic Runtime:** Progress rendering and execution model can be adapted for Discord, Slack, etc.
- **Topic-Scoped Infrastructure:** Per-topic agent profiles, hooks, tools, and memory isolation
- **Manager Control Plane:** Programmatic topic management with RBAC, audit trail, and rollback support
- **Sandbox Broker:** Security isolation with Unix socket broker (`oxide-agent-sandboxd`)
</details>

## Features

*   **🏗️ Workspace Architecture:** Modular crate design with clear separation of concerns:
    - `oxide-agent-core` - Domain logic, LLM integrations, hooks, skills, storage
    - `oxide-agent-runtime` - Session orchestration, execution cycle, tool providers, sandbox
    - `oxide-agent-transport-telegram` - Telegram transport layer (teloxide integration)
    - `oxide-agent-transport-web` - E2E testing infrastructure with HTTP API
    - `oxide-agent-sandboxd` - Sandbox broker daemon for Docker access isolation
    - `oxide-agent-telegram-bot` - Binary entry point and configuration

*   **🤖 Agent Mode:**
        <img width="974" height="747" alt="image_2026-01-11_20-58-21" src="https://github.com/user-attachments/assets/c99e55e4-8933-4ec8-9f50-22f7cbca4c77" />

    *   **Integrated Sandbox:** Safe execution of Python code and Bash commands in isolated Docker containers (`debian:trixie-slim`).
    *   **Parallel Tool Execution:** Multiple tool calls in one LLM response execute concurrently for faster task completion.
    *   **Fire-and-Forget Checkpoint:** Memory persistence is async, non-blocking for reduced latency.
    *   **History Repair:** Validates tool_call_id before LLM calls; orphaned tool results prevented during compaction.
    *   **Cold-Start Tool Drift Pruning:** Removes stale tool calls from persisted memories on startup.
    *   **Tools:** Read/write files, execute commands, web search, work with video and file hosting.
    *   **📋 Task Management (Todos):** `write_todos` system for planning and tracking progress of complex requests.
    *   **🎯 Skills System:** RAG system with embeddings to automatically provide relevant context from markdown documents (9 skills: core, delegation_manager, ffmpeg-conversion, file-hosting, file-management, html-report, task-planning, video-processing, web-search).
    *   **📁 File Handling:** Accept files from user (up to 20MB), send to Telegram (up to 50MB), or upload to cloud (up to 4GB) with link generation.
    *   **🎬 Video Processing:** `yt-dlp` integration for downloading video and media files from the internet.
        <img width="977" height="762" alt="image" src="https://github.com/user-attachments/assets/1ffb66b7-559b-453f-9330-fbe27ccee90e" />

    *   **☁️ File Hosting:** Upload files from sandbox to public hosting with short retention time.
    *   **Web Search and Data Extraction:** Multiple independent search providers — SearXNG (self-hosted, default), Tavily (API), Crawl4AI (deep crawling) — can run simultaneously.
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
    *   **Long-term Memory and Context:** Up to 200K tokens with automatic compression when limit reached.
    *   **🗣️ Narrator:** Separate model for summarizing agent thoughts and actions in chat.
    *   **Execution Progress:** Interactive display of current working step in Telegram.
*   **Multi-LLM Support:** 5 main providers for Chat/Agent mode (Zhipu AI/ZAI, MiniMax, OpenRouter, Mistral, Google Gemini). Groq is supported in **Chat Mode only**.
*   **Native Tool Calling:** Efficient use of tools in modern models with ToolCallCorrelation architecture.
*   **Multimedia Processing:**
    *   Voice and video messages (speech recognition via Gemini or Voxtral).
    *   Images (analysis and description via multimodal models).
    *   Work with documents of various formats.
*   **🗣️ Voice Synthesis:** Kokoro TTS integration for generating voice messages from agent output.
*   **Context Management:** Dialogue history saved in Cloudflare R2 (S3) with context-scoped isolation per topic.
*   **🔒 Security and Quality:** `unsafe_code = "forbid"`, strict Clippy lints, no panics (`zero-panic profile`), DM tool restrictions, SSH approval flow, RBAC.

## System Requirements

<details>
<summary>🔑 API Keys and Infrastructure</summary>

### 🔑 API Keys (Mandatory)
| Provider | Variable | Description |
| :--- | :--- | :--- |
| **Zhipu AI (ZAI)** | `ZAI_API_KEY` | **Mandatory for Agent** (`glm-4.7`, Default Agent Model). [Zhipu AI](https://z.ai/) |
| **Telegram** | `TELEGRAM_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) |
| **Cloudflare R2** | `R2_*` | S3 storage (Access Key, Secret, Endpoint, Bucket) |
| **Mistral AI** | `MISTRAL_API_KEY` | **Critical for Agent** (`mistral-embed` model for skill selection) |

### 🤖 Supported LLM Providers for Chat/Agent Mode
The bot supports **5 main providers** for both standard chat and advanced Agent mode (with tool calling):

*   **Zhipu AI / ZAI** (`ZAI_API_KEY`) — primary provider for Agent Mode (`glm-4.7` or `glm-4.5-air`). Provides native tool-aware chat completions and reasoning.
*   **MiniMax** (`MINIMAX_API_KEY`) — Claude SDK-compatible provider via MiniMax API (`MiniMax-M2.7`).
*   **OpenRouter** (`OPENROUTER_API_KEY`) — commonly used for chat/multimodal requests (e.g., `google/gemini-3-flash-preview`). Supports tool calling for Agent mode through compatible models. Ensure `CHAT_MODEL_PROVIDER=openrouter` if you need Gemini voice/image support.
*   **Mistral** (`MISTRAL_API_KEY`) — great for cost-effective agent/chat combos (e.g., `mistral-large-latest`, `pixtral-large-latest`). Supports tool calling via JSON mode or native tools. Includes Voxtral audio transcription (`voxtral-mini-latest`).
*   **Google Gemini** (`GEMINI_API_KEY`) — direct integration for Gemini models, primarily used for multimodal tasks or as a fallback.

#### Other Providers (Chat only)
*   **Groq** (`GROQ_API_KEY`) — optional provider for fast specialized chat workloads (e.g. `llama-3.3-70b-versatile`).

> [!NOTE]
> Voice recognition and image analysis depend on whichever multimodal model you configure via `CHAT_MODEL_*`/`MEDIA_MODEL_*`. The bot exposes only the models you declare in `.env`, so `Change Model` will only list those names.

### 🛠 Infrastructure
*   **Docker** — run code sandbox (`agent-sandbox:latest`)
*   **Sandbox Broker** — optional Unix socket broker for security isolation (`SANDBOX_BACKEND=broker`)
*   **Tavily API** — optional web search provider (`TAVILY_API_KEY`)
*   **SearXNG** — self-hosted search engine, runs as Docker sidecar (`SEARXNG_URL`)
*   **Crawl4AI** — deep web crawling provider with markdown extraction and PDF parsing capabilities
*   **Kokoro TTS Server** — optional for voice message synthesis (`KOKORO_TTS_URL`)
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

**Note:** The default configuration uses `SANDBOX_BACKEND=broker` which requires the `oxide-agent-sandboxd` container. To use direct Docker access, set `SANDBOX_BACKEND=docker`.
</details>

## Configuration (.env)

<details>
<summary>⚙️ Example Configuration File</summary>

```dotenv
# Telegram
TELEGRAM_TOKEN=YOUR_TOKEN
ALLOWED_USERS=ID1,ID2 # List of allowed Telegram IDs (basic access)
AGENT_ACCESS_IDS=ID1 # Access to Agent Mode (consumes many tokens)

# Agent Configuration
AGENT_TIMEOUT_SECS=300          # Agent execution timeout
SEARCH_PROVIDER=tavily          # [DEPRECATED] use TAVILY_ENABLED / SEARXNG_ENABLED / CRAWL4AI_ENABLED
DEBUG_MODE=false                # Debug logging mode

# Cloudflare R2 (S3)
R2_ACCESS_KEY_ID=...
R2_SECRET_ACCESS_KEY=...
R2_ENDPOINT_URL=...
R2_BUCKET_NAME=...
R2_REGION=auto                 # S3-compatible storage region

# API Keys
GROQ_API_KEY=...
MISTRAL_API_KEY=...
GEMINI_API_KEY=...
OPENROUTER_API_KEY=...
ZAI_API_KEY=...                 # Zhipu AI / ZAI Provider
MINIMAX_API_KEY=...             # MiniMax Provider (Claude SDK-compatible)
TAVILY_API_KEY=...             # Tavily web search in Agent mode (optional, enable via TAVILY_ENABLED=true)
SEARXNG_URL=http://127.0.0.1:8081  # SearXNG self-hosted search (auto-enabled when set)
SEARXNG_ENABLED=true            # Explicit toggle for SearXNG provider
CRAWL4AI_ENABLED=true           # Enable Crawl4AI deep crawling provider
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

### Weighted Model Routes (Failover)
Configure multiple weighted routes for automatic failover after persistent 429 errors:

```dotenv
# Priority: MiniMax > ZAI > Mistral
AGENT_MODEL_ROUTES__0__ID="MiniMax-M2.7"
AGENT_MODEL_ROUTES__0__PROVIDER="minimax"
AGENT_MODEL_ROUTES__0__WEIGHT=10

AGENT_MODEL_ROUTES__1__ID="glm-4.7"
AGENT_MODEL_ROUTES__1__PROVIDER="zai"
AGENT_MODEL_ROUTES__1__WEIGHT=5

AGENT_MODEL_ROUTES__2__ID="mistral-small-2603"
AGENT_MODEL_ROUTES__2__PROVIDER="mistral"
AGENT_MODEL_ROUTES__2__WEIGHT=2
```

### Alternate provider example
```
CHAT_MODEL_ID="mistral-large-latest"
CHAT_MODEL_PROVIDER="mistral"

AGENT_MODEL_ID="devstral-2512"
AGENT_MODEL_PROVIDER="mistral"
```

Repeat the `_MODEL_ID/_MODEL_PROVIDER` pattern for Groq, Gemini-specific IDs, or other providers you want to expose. Only set names will be available in the chat mode keyboard.

## Available LLM Providers

| Name | Provider | Features |
| :--- | :--- | :--- |
| **OR Gemini 3 Flash** | OpenRouter | Multimodal, default chat model |
| **ZAI GLM-4.7** | ZAI (Zhipu AI) | Default agent model, GLM Coding Plan |
| **MiniMax M2.7** | MiniMax | Claude SDK-compatible, high context |
| **Mistral Large** | Mistral | Free and generous, includes Voxtral audio transcription |
| **Gemini 2.5 Flash Lite** | Google | Cheap and efficient |
| **Devstral 2512** | Mistral | Top free for coding and agent work |

> **Note:** The models listed above are recommended configurations. Only models declared in your `.env` file will be available in the bot's "Change Model" menu.

## New Tool Providers

### 🗣️ Kokoro TTS (Voice Synthesis)
Generates voice messages from agent output using local Kokoro TTS server.

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

### 🔌 Jira MCP Integration
Full Jira Server 7.5.0 integration via MCP protocol.

**Configuration:**
```dotenv
JIRA_URL=https://jira.company.com
JIRA_EMAIL=agent@company.com
JIRA_API_TOKEN=your_api_token
JIRA_MCP_BINARY_PATH=/usr/local/bin/jira-mcp  # Auto-detected
```

**Feature Flag:** `--features jira`

**Tools:** `jira_read`, `jira_write`, `jira_schema` (disabled by default)

**Usage:** Enable via `topic_agent_tools_enable` with `tools=["jira"]`

### 💬 Mattermost MCP Integration
Mattermost workspace integration via MCP protocol (16 tools).

**Configuration:**
```dotenv
MATTERMOST_URL=https://mattermost.company.com
MATTERMOST_TOKEN=your_bot_or_user_token
MATTERMOST_MCP_BINARY_PATH=/usr/local/bin/mattermost-mcp  # Auto-detected
```

**Feature Flag:** `--features mattermost`

**Tools:** Teams, channels, messages, users, files operations

### 🔐 SSH MCP Infrastructure
Topic-scoped SSH tools with approval flow for sensitive operations.

**Configuration:**
```dotenv
OXIDE_SSH_MCP_BINARY=/usr/local/bin/ssh-mcp
```

**Tools:** `ssh_exec`, `ssh_sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_check_process`

**Features:**
- Approval flow with TTL 600s
- Secret refs: `env:KEY` and `storage:PATH`
- Dangerous commands require approval (sudo, file edits on sensitive paths)

**Blocked in DM:** All SSH tools are blocked in private/DM chats by default.

## Manager Control Plane

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
MANAGER_ALLOWED_USERS=123456789,987654321  # Users with manager control-plane access
MANAGER_HOME_CHAT_ID=-1001234567890        # Restrict to specific chat (optional)
MANAGER_HOME_THREAD_ID=1                   # Thread ID (optional)
MANAGER_HOME_AGENT_ID=control-plane       # Agent ID for manager home (optional)
```

**Note:** When `MANAGER_HOME_CHAT_ID` is set, manager control-plane tools are only available in the designated topic.

## Security

### DM Tool Restrictions
SSH, Jira, and Mattermost tools are **blocked by default in private/DM chats** for security.

**Configuration:**
```dotenv
DM_ALLOWED_TOOLS=todos_write,todos_list,delegate_to_sub_agent  # Allowlist mode
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

## Breaking Changes

<details>
<summary>⚠️ Important Changes from Previous Versions</summary>

### 1. DM Tool Restrictions (March 23, 2026)
SSH, Jira, and Mattermost tools are now **blocked by default in private/DM chats**.

**Migration:** If you need these tools in DMs, configure `DM_ALLOWED_TOOLS` or `DM_BLOCKED_TOOLS` env vars.

### 2. Sandbox Broker Default
`SANDBOX_BACKEND=broker` is now the default for security isolation.

**Migration:** Ensure `oxide-agent-sandboxd` container is running, or set `SANDBOX_BACKEND=docker` for direct Docker access.

### 3. Cold-Start Tool Drift Pruning
Startup maintenance sweep that removes stale tool calls from persisted memories.

**Migration:** Monitor first startup after upgrade for memory rewrites. Disable with `STARTUP_TOOL_DRIFT_PRUNE_ENABLED=false` if needed.

### 4. Compaction Token-Based Window
Token-based protected tool window instead of fixed count.

**Migration:** Adjust `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS` (default: 8192) if needed. Recommended: 12k-16k for DevOps workflows.
</details>

## Agent Architecture


<details>
<summary>🏗 Internal Structure, Skills, Hooks, Compaction</summary>

### 🎯 Skills System
The agent uses a RAG approach with embeddings to automatically provide relevant context:
- **9 skills** as markdown documents (`skills/`)
- **Semantic matching** of user requests with skills via cosine similarity (threshold: 0.6)
- **Embeddings caching** for fast access
- **Automatic injection** of relevant instructions into the system prompt
- **Configuration:** `SKILL_TOKEN_BUDGET` (4096 tokens), `SKILL_SEMANTIC_THRESHOLD` (0.6), `SKILL_MAX_SELECTED` (3)

**Available Skills:**
- `core` - Basic agent rules (always loaded)
- `delegation_manager` - Sub-agent delegation patterns
- `file-management` - Sandbox operations
- `file-hosting` - Large file uploads
- `task-planning` - Todo management for multistep tasks
- `web-search` - Web search and extraction tools
- `video-processing` - YT-DLP integration
- `ffmpeg-conversion` - Media conversion
- `html-report` - Design style guide

### 🔄 Loop Protection
Three-level loop detection system (`agent/loop_detection/`):
1. **Content Detector** — analyzes repeating agent messages
2. **Tool Detector** — tracks identical tool calls
3. **LLM Detector** — uses LLM to analyze loop patterns

**Configuration:** `LOOP_DETECTION_ENABLED`, `LOOP_TOOL_CALL_THRESHOLD` (5), `LOOP_LLM_CHECK_AFTER_TURNS` (30), `LOOP_SCOUT_MODEL`

### 🔄 Compaction Pipeline
Advanced context compression with token-based protected window:
1. **Budget Estimation** - Estimate memory usage
2. **Classify** - Categorize messages by importance
3. **Externalize** - Move payloads to archive
4. **Prune** - Remove less important messages (respects protected window)
5. **Summarize** - Generate concise summary with retry backoff
6. **Rebuild** - Reconstruct hot context

**Configuration:**
- `COMPACTION_PROTECTED_TOOL_WINDOW_TOKENS` (8192 tokens)
- `COMPACTION_MODEL_ID`, `COMPACTION_MODEL_PROVIDER` - Dedicated LLM for summarization
- `COMPACTION_MODEL_MAX_OUTPUT_TOKENS` (512)
- `COMPACTION_MODEL_TIMEOUT_SECS` (20)

### 🔗 Hooks System
Extensible architecture for personalizing agent behavior:
- **Completion Hook** — task completion handling
- **Workload Distributor** — enforces separation of duties by blocking heavy manual operations in the Main Agent and encouraging delegation
- **Delegation Guard** — prevents delegation of high-level analytical tasks ("think", "analyze"), restricting sub-agents to mechanical retrieval
- **Sub-Agent Safety** — ensures safe execution environments for delegated tasks
- **Search Budget** — limits search tool calls (10 per session)
- **Timeout Report** — provides detailed timeout reporting
- **Tool Access Policy** — blocks tools not allowed by current profile

**Protected Hooks (cannot be disabled):** `completion_check`, `tool_access_policy`

**Manageable Hooks:** `workload_distributor`, `delegation_guard`, `search_budget`, `timeout_report`

### 🛠️ Tool Providers
The agent uses a modular provider system, each offering a specialized set of tools:
- **Sandbox Provider** (`sandbox.rs`) — code execution, file read/write, shell commands
- **SearXNG Provider** (`searxng/`) — self-hosted web search via JSON API
- **Tavily Provider** (`tavily.rs`) — web search and data extraction
- **Crawl4AI Provider** (`crawl4ai.rs`) — deep web crawling with markdown extraction and PDF parsing (retry with backoff, concurrency limit)
- **Todos Provider** (`todos.rs`) — task list management for long-term planning
- **YT-DLP Provider** (`ytdlp.rs`) — video and audio download from various platforms
- **File Hoster Provider** (`filehoster.rs`) — public file upload to temporary hosting (up to 4GB)
- **Delegation Provider** (`delegation.rs`) — sub-agent delegation for complex task decomposition
- **Reminder Provider** (`reminder.rs`) — reminder scheduling with pause/resume/retry
- **Kokoro TTS Provider** (`tts/`) — voice message synthesis
- **Jira MCP Provider** (`jira_mcp/`) — Jira integration
- **Mattermost MCP Provider** (`mattermost_mcp/`) — Mattermost integration
- **SSH MCP Provider** (`ssh_mcp.rs`) — SSH infrastructure with approval flow
- **Manager Control Plane** (`manager_control_plane/`) — Topic CRUD, RBAC, audit trail
- **Agents MD Provider** (`agents_md.rs`) — Topic-scoped AGENTS.md editing

### 🚀 Performance Optimizations
- **HTTP Connection Pooling:** Shared HTTP client for all LLM providers (reduces latency)
- **Tokenizer Caching:** cl100k tokenizer cached at startup (~15s latency eliminated)
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

1. **sandbox_image** (profile: "build")
   - Builds agent-sandbox image from `sandbox/Dockerfile.sandbox`
   - Used for pre-warming sandbox containers

2. **oxide_agent** (main bot)
   - Builds from root Dockerfile
   - Network mode: `host`
   - Mounts: `./config:/app/config`, `sandboxd-run:/run/sandboxd`
   - Environment: `SANDBOX_BACKEND=broker`, `SANDBOXD_SOCKET=/run/sandboxd/sandboxd.sock`

3. **sandboxd** (broker daemon)
   - Builds from root Dockerfile
   - Command: `./oxide-agent-sandboxd`
   - Runs as user 0 (privileged for Docker access)
   - Mounts: `/var/run/docker.sock:/var/run/docker.sock` (only sandboxd has Docker access)
   - Socket: `/run/sandboxd/sandboxd.sock`

4. **crawl4ai** (web crawler)
    - Image: `unclecode/crawl4ai:0.8.5`
    - Health check: `curl -f http://localhost:11235/health`
    - Resources: 6GB RAM, 4 CPUs, 2GB shared memory

5. **searxng** (self-hosted search)
    - Image: `searxng/searxng:2026.3.24-054174a19`
    - Port: `127.0.0.1:8081:8080`
    - Health check: `wget -qO- http://localhost:8080/healthz`
    - Config: `docker/searxng/settings.yml`

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
├── oxide-agent-core/           # Domain logic, LLM integrations, hooks, skills, storage
│   └── src/
│       ├── agent/              # Agent core and execution logic
│       │   ├── compaction/     # Compaction pipeline (12 modules)
│       │   ├── hooks/          # Execution hooks (7 hooks)
│       │   ├── loop_detection/ # Loop detection (content, tool, llm)
│       │   ├── providers/      # Tool providers (12 providers)
│       │   │   ├── ssh_mcp.rs            # SSH infrastructure
│       │   │   ├── jira_mcp/             # Jira integration
│       │   │   ├── mattermost_mcp/       # Mattermost integration
│       │   │   ├── tts/                  # Kokoro TTS
│       │   │   ├── manager_control_plane/ # Topic CRUD, RBAC
│       │   │   └── ...
│       │   ├── recovery/       # History repair, tool drift pruning
│       │   ├── runner/         # Execution loop, parallel tools
│       │   └── skills/         # Skills subsystem (RAG/embeddings)
│       ├── llm/                # LLM provider integrations
│       │   ├── providers/      # 5+ providers (zai, minimax, mistral, gemini, groq)
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

skills/                         # Skill definitions (markdown)
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

<details>
<summary>💻 Developer Commands and CI/CD</summary>

For local development (requires Rust installed):
```bash
# Check
cargo check

# Testing
cargo test --workspace

# Linting (Clippy with warn/deny)
cargo clippy --workspace --tests -- -D warnings

# Formatting
cargo fmt --all

# Build with feature flags
cargo build --release --features searxng,crawl4ai,jira,mattermost

# Run E2E tests (requires transport-web crate)
cargo test -p oxide-agent-transport-web --test e2e
```

### Testing Infrastructure

The project uses a comprehensive testing approach:

- **Hermetic Tests:** Isolated tests using mock implementations
- **E2E Tests:** Full end-to-end tests via web transport (`crates/oxide-agent-transport-web/tests/e2e/`)
  - Session lifecycle, SSE streaming
  - Compaction regression, delegation
  - Reminder system, tool latency benchmarks
  - Live ZAI audit tests (requires `RUN_LLM_E2E_CHECKS=1`)
- **Test Utilities:** Helper functions for quick mock setup

**Testing Dependencies:**
- `mockall` (0.14.0) - Trait-based mocking framework
- `insta` (1.46.1) - Snapshot testing framework

### CI/CD

The project uses GitHub Actions for automatic testing and deployment:
- **Testing:** Runs `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt`
- **Validation:** Integration tests with real API keys (push to non-PR branches)
- **Deployment:** Automatic deploy to server via SSH, dynamic docker-compose generation
- **Docker:** Multi-platform builds with Docker Buildx, pushes to Docker Hub on `testing` branch

### Security and Lints

- **`unsafe_code = "forbid"`** in workspace lints — unsafe code is forbidden
- **Clippy lints (forbid level):**
  - `unwrap_used = "forbid"` — all Result/Option must be handled via `?` or `match`
  - `too_many_lines = "forbid"` — files >300 lines must be split
  - `too_many_arguments = "forbid"` — functions >3 arguments require Context/Config struct
- **Feature flags:** Tavily, SearXNG, Crawl4AI, Jira, Mattermost available via `--features`
- **Error Handling:** Using `thiserror` for library errors, `anyhow` for application
</details>

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `tavily` | Enable Tavily web search provider | Enabled |
| `searxng` | Enable SearXNG self-hosted search provider | Enabled |
| `crawl4ai` | Enable Crawl4AI web search provider | Disabled |
| `jira` | Enable Jira MCP integration | Disabled |
| `mattermost` | Enable Mattermost MCP integration | Disabled |

Build with features:
```bash
cargo build --release --features searxng,crawl4ai,jira,mattermost
```

## License

The project is distributed under the **GNU Affero General Public License v3 (AGPL-3.0)**. Details in the [LICENSE](https://github.com/0FL01/oxide-agent/blob/main/LICENSE) file.

Copyright (C) 2026 @0FL01
