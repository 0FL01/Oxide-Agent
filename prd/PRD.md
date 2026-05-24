# PRD: Modular Architecture Refactor for Oxide Agent

## 1. Executive Summary

Oxide Agent must be refactored from the current monolithic/partially modular architecture into a capability-oriented modular architecture. Every provider, tool, storage backend, sandbox backend, transport, MCP integration, media feature, search/browser integration, and sidecar service must become an explicit capability that can be included or excluded at compile time and then enabled or disabled at runtime only if it was compiled into the binary.

This is a clean-slate refactor inside the current repository. Backward compatibility is not required. Migrations are not required. Legacy registries, compatibility shims, deprecated wrappers, duplicated registration paths, old environment variable aliases, and “keep this just in case” execution paths must be removed rather than wrapped.

The target architecture must satisfy these hard constraints:

- no backward compatibility layer;
- no deployment migration layer;
- no legacy tool registry;
- no duplicated typed/legacy/runtime registration;
- no unused dependencies in minimal builds;
- no unused LLM provider SDKs in provider-specific builds;
- no AWS SDK unless the selected `storage-s3-r2` module requires it;
- no durable local-storage target;
- no Docker/Bollard dependency unless a Docker sandbox backend is selected;
- no MCP binaries unless a specific MCP integration is selected;
- no browser/search/media sidecars unless the selected profile requires them;
- no fat sandbox image for lite or embedded profiles;
- full modularity for tools, providers, transports, storage, sandbox, MCP, media, search, browser, memory, reminders, and file delivery.

The final result must allow deterministic builds such as:

```bash
cargo build --release \
  --no-default-features \
  --features "profile-embedded-opencode-local"
```

and produce a binary/container that contains only the modules required by that profile.

## 2. Goals

The refactor has the following goals.

### 2.1 Deterministic build profiles

A build profile must fully determine which Rust modules, dependencies, binaries, Docker sidecars, sandbox image packages, and runtime capabilities are available. Runtime configuration may disable compiled capabilities, but it must not enable capabilities that were not compiled.

### 2.2 Minimal embedded builds

The repository must support a small embedded profile with only a selected transport, one selected LLM provider, the canonical S3/R2 durable storage backend, and a small selected tool set. The embedded profile must not compile unused LLM providers, unselected storage backends, MCP integrations, browser tools, searxng integrations, Docker sandbox execution, or media-heavy dependencies unless explicitly requested.

### 2.3 Arbitrary custom builds

A downstream builder must be able to create a custom profile such as:

- only OpenCode Go provider;
- only the canonical S3/R2 durable storage backend;
- only Telegram transport;
- no sandbox;
- sandbox file operations but no command execution;
- search tools but no browser sidecar;
- Jira MCP but not Mattermost MCP;
- image analysis but not audio/video processing;
- transient local workspace with no durable local storage;
- CLI/API transport with no Telegram dependency.

### 2.4 Single source of truth for module registration

Tool, provider, storage, sandbox, and transport registration must happen through one module registry. There must be no split between legacy provider registration and typed runtime registration. A module must register itself and its executors/providers in one place.

### 2.5 Compile-time dependency elimination

Every heavy or optional dependency must be behind an atomic Cargo feature. Profile features must be compositions of atomic features. Minimal builds must prove via `cargo tree` checks that excluded modules do not leak their dependencies.

Heavy dependency groups include, but are not limited to:

- AWS SDK / S3/R2 storage module;
- Bollard / Docker sandbox management;
- RMCP / child-process MCP clients;
- Telegram / teloxide;
- browser-use sidecar integration;
- searxng integration;
- Tavily SDK;
- OpenAI-compatible SDKs;
- ZAI SDK;
- Groq/Mistral/Minimax/Nvidia/OpenRouter provider code;
- media/audio/image/video processing support;
- yt-dlp / ffmpeg / browser / Chromium sidecars;
- SSH MCP support;
- Jira and Mattermost MCP binaries.

### 2.6 Runtime config for compiled modules only

Configuration must be validated against the compiled capability manifest. Unknown module IDs and config for non-compiled modules must fail during startup with a clear error.

### 2.7 Clean provider and tool abstraction

LLM providers and tools must be independent modules with explicit IDs, config schemas, capabilities, dependencies, and registration functions. Global hardcoded provider match chains must be replaced with provider modules and factories.

### 2.8 Modular Docker and Compose

Dockerfiles and Compose files must reflect the selected module set. They must not be the source of architectural modularity. Docker must select Cargo features and runtime assets for a profile; it must not ship everything and rely on runtime flags.

### 2.9 Measurable size and dependency improvements

The refactor must add CI checks that measure binary size, container image size, compiled capabilities, and dependency leakage for key profiles.

### 2.10 Easy module addition

Adding a new tool must require adding one module and one feature, plus optional profile inclusion. It must not require editing multiple registries, typed wrappers, legacy wrappers, and global transport startup chains.

Adding a new provider must require adding one provider module and one feature, plus optional profile inclusion. It must not require editing multiple global match chains.

## 3. Non-Goals

The following are explicitly out of scope.

- Preserving old config compatibility is not a goal.
- Migrating existing deployments is not a goal.
- Keeping legacy execution paths is not a goal.
- Supporting old registry APIs is not a goal.
- Hiding the old architecture behind adapters is not a goal.
- Keeping old environment variable aliases is not a goal.
- Keeping deprecated config fields is not a goal.
- Keeping full Docker Compose as the default for all profiles is not a goal.
- Keeping R2 as a hardcoded storage backend is not a goal.
- Keeping all LLM providers compiled into every binary is not a goal.
- Keeping sandbox command execution available by default is not a goal.
- Keeping MCP binaries in the runtime image unless selected is not a goal.
- Keeping a fat sandbox image for every deployment is not a goal.
- Maintaining old tool names only for compatibility is not a goal.
- Preserving current file layout is not a goal.
- Preserving current Cargo feature names is not a goal.
- Preserving current Dockerfile names or Compose topology is not a goal.

## 4. Current Architecture RECON

This section describes the observed state of the current repository and the concrete architectural problems that must be removed.

### 4.1 Workspace and binaries

The current workspace contains these crates:

- `crates/oxide-agent-core`
  - Core domain crate.
  - Contains agent execution, config, LLM providers, tool providers, sandbox management, storage, reminders, wiki memory, skill loading, hooks, compaction, and many provider-specific integrations.
  - Currently carries too many responsibilities and too many unconditional dependencies.

- `crates/oxide-agent-runtime`
  - Runtime transport/session abstraction.
  - Depends on `oxide-agent-core`.
  - Provides reusable session/progress/runtime primitives for transports.

- `crates/oxide-agent-transport-telegram`
  - Telegram transport implementation using `teloxide`.
  - Contains bot handlers, routing, progress handling, reminder scheduling integration, manager topic handling, and application startup orchestration.
  - Currently hardcodes R2 storage initialization and creates global runtime services from one transport-specific runner.

- `crates/oxide-agent-transport-web`
  - HTTP/SSE-style transport primarily useful for tests and in-memory scenarios.
  - Uses in-memory storage and scripted LLM helpers.
  - It shows that transport abstraction exists, but production startup is still dominated by Telegram and R2 assumptions.

- `crates/oxide-agent-telegram-bot`
  - Main Telegram bot binary crate.
  - Loads `.env`, configures tracing, redacts secrets, performs SSH temp-file cleanup, creates `BotSettings`, and starts the Telegram runner.
  - Also contains `src/bin/chatgpt-login.rs`.

- `crates/oxide-agent-sandboxd`
  - Sandbox broker daemon binary.
  - Starts the sandbox broker server over Unix socket.
  - Also performs SSH private key cleanup even though SSH cleanup is not a sandbox backend responsibility.

Direct Gemini SDK usage was removed from the target architecture; Gemini-family models are reached through OpenRouter instead of a vendored SDK.

Current binary-level problem:

- `oxide-agent-telegram-bot` and `oxide-agent-sandboxd` are built together in the main Dockerfile.
- The Docker image also copies `chatgpt-login`, MCP binaries, and skills into the same runtime image.
- Binaries are not selected by module profile.
- Transport startup currently acts as application composition.

Target state:

- The main application binary must be composed from selected capabilities.
- Transports must not decide storage, provider, sandbox, or MCP topology.
- Each binary must declare required features.
- Optional binaries must not be built or copied unless selected.

### 4.2 Current Docker and Compose topology

The current Docker topology is full-profile by default.

The root `Dockerfile`:

- uses cargo-chef for dependency caching;
- builds `oxide-agent-telegram-bot` and `oxide-agent-sandboxd` together;
- enables `oxide-agent-core/jira` and `oxide-agent-core/mattermost` during dependency and app builds;
- downloads and copies `ssh-mcp`, `jira-mcp`, and `mattermost-mcp` binaries into the final image unconditionally;
- installs runtime packages such as `openssh-client`, `libssl3`, `tzdata`, and CA certificates;
- copies the Telegram bot binary, sandbox daemon binary, `chatgpt-login`, MCP binaries, and skills into one image;
- uses a single runtime image for both the bot and sandbox daemon.

The current `docker-compose.yml`:

- builds `sandbox/Dockerfile.sandbox` as `agent-sandbox:latest`;
- starts `oxide_agent` from the full runtime image;
- sets `SANDBOX_BACKEND=broker`;
- starts `sandboxd` as a separate service with Docker socket access;
- starts `searxng` unconditionally and wires `SEARXNG_ENABLED=true` into `oxide_agent`;
- declares `oxide_agent` as depending on `sandboxd` and `searxng`;
- contains a commented browser-use service rather than a selected module-driven service;
- uses host networking for the main bot and sandbox daemon;
- creates volumes even when a profile does not need the corresponding service.

The current sandbox image `sandbox/Dockerfile.sandbox` is a fat universal image. It installs general networking tools, Python, pip, ffmpeg, yt-dlp, requests/httpx/BeautifulSoup/lxml, nmap, mtr, traceroute, ripgrep, fd, jq, git, zip/unzip, and related utilities regardless of which tools are enabled.

The current browser sidecar under `services/browser_use_bridge` is also heavy. It installs Chromium, browser-use, FastAPI, and runtime dependencies. It is not currently selected by a capability manifest.

Current Docker/Compose problems:

- Docker always assumes full application topology.
- Jira and Mattermost features are hardcoded into image builds.
- MCP binaries are copied even when integrations are not used.
- The same image is used for bot and sandbox daemon.
- `searxng` is always present in compose.
- `sandboxd` is always present in compose.
- The sandbox image is heavy even when command execution/media tooling is not selected.
- Docker/Compose express deployment assumptions rather than selected capabilities.

Target state:

- Docker builds must accept a profile or explicit feature list.
- Runtime images must contain only selected binaries/assets.
- Compose files must be profile-specific or generated from module service requirements.
- Sandbox image variants must match selected sandbox and media modules.

### 4.3 Current tool registration flow

There are two distinct tool registration systems today.

#### Legacy provider registry

`oxide-agent-core/src/agent/registry.rs` defines a legacy `ToolRegistry` around `Vec<Box<dyn ToolProvider>>`.

`oxide-agent-core/src/agent/provider.rs` defines `ToolProvider`, which exposes:

- provider name;
- tool definitions;
- `can_handle`;
- `execute`.

The legacy registry flattens provider tool definitions and dispatches tool execution by iterating providers.

#### Typed runtime registry

`oxide-agent-core/src/agent/tool_runtime/registry.rs` defines a separate typed `ToolRegistry` around `BTreeMap<ToolName, Arc<dyn ToolExecutor>>`.

The typed runtime has separate concepts for:

- `ToolExecutor`;
- `ToolInvocation`;
- `ToolOutput`;
- normalization;
- process/runtime execution;
- provider-specific protocol conversion.

#### Registration glue and duplication

`oxide-agent-core/src/agent/executor/registry.rs` chooses between the typed runtime and the legacy registry depending on model/runtime checks.

It contains separate flows:

- `build_tool_registry(...)` for the legacy provider registry;
- `build_tool_runtime_registry(...)` for the typed runtime registry.

The legacy flow registers broad providers and integrations:

- core providers such as todos, sandbox, compression, stack logs, file hosting, media file, yt-dlp, and delegation;
- topic providers such as agents.md, manager control plane, SSH MCP, and reminders;
- wiki memory provider;
- Jira and Mattermost MCP providers behind features and runtime config;
- Tavily, SearXNG, and WebFetchMd search/fetch providers;
- browser-use providers behind feature and runtime config;
- Kokoro and Silero TTS providers based on URL env/config behavior.

The typed runtime flow registers only part of the same surface:

- typed todos executors;
- typed sandbox executors;
- selected topic/runtime providers;
- wiki memory executors;
- SSH MCP executors.

It also includes a `ProviderRuntimeExecutor` adapter that wraps a legacy `ToolProvider` as a typed `ToolExecutor`.

Current tool registration problems:

- There are two registries.
- Some tools are registered in both systems.
- Some tools exist only in the legacy path.
- Some tools are wrapped from legacy into typed runtime rather than being first-class typed modules.
- Registration behavior depends on runtime model checks instead of compiled capability manifests.
- Duplicate registration is handled by skipping duplicates instead of eliminating duplicate registration paths.
- Tool availability is mostly runtime-filtered, not compile-time eliminated.
- Broad providers mix unrelated capabilities.

Tools and provider groups currently observed include:

- `SandboxToolProvider`
  - `execute_command`
  - `write_file`
  - `read_file`
  - `send_file_to_user`
  - `list_files`
  - `recreate_sandbox`

- `TodosProvider`
  - todo/list state tools.

- `CompressionToolProvider`
  - `compress`.

- `DelegationToolProvider`
  - `spawn_sub_agents`
  - `wait_sub_agents`
  - `cancel_sub_agents`

- `FileHosterProvider`
  - `upload_file`.

- `MediaFileToolProvider`
  - `transcribe_audio_file`
  - `describe_image_file`
  - `describe_video_file`

- `YtdlpToolProvider`
  - video metadata, transcript, search, video download, audio download.

- `AgentsMdProvider`
  - `agents_md_get`
  - `agents_md_update`

- `ReminderToolProvider`
  - reminder schedule/list/cancel/pause/resume/retry tools.

- `WikiMemoryProvider`
  - wiki memory list/read/delete tools.

- `WebFetchMdProvider`
  - `web_markdown`.

- `TavilyToolProvider`
  - `web_search`
  - `web_extract`.

- `SearxngToolProvider`
  - `searxng_search`.

- `BrowserUseToolProvider`
  - browser task/session/content/screenshot tools.

- `JiraMcpProvider`
  - Jira read/write/schema tools.

- `MattermostMcpProvider`
  - Mattermost channel/post/team/user/file/reaction/search/status tools.

- `SshMcpProvider`
  - SSH exec, sudo exec, read file, file edit, process check, send file tools.

- `ManagerControlPlane` providers
  - forum topic tools;
  - topic binding/context/agents.md/infra/sandbox tools;
  - agent profile tools;
  - tool/hook enable-disable tools;
  - private secret probing.

- `KokoroTtsProvider`
  - English TTS tools.

- `SileroTtsProvider`
  - Russian TTS tools.

Target state:

- Delete the legacy provider registry.
- Delete the typed/legacy adapter.
- Delete dual build paths.
- Every tool must be registered by exactly one capability module.
- A tool definition and its executor must live together.
- Broad providers must be split into atomic capability modules.

### 4.4 Current LLM provider flow

The current LLM provider architecture is centralized in `oxide-agent-core`.

`oxide-agent-core/src/llm/provider.rs` defines `LlmProvider`, with methods for:

- chat completion;
- audio transcription;
- audio transcription with prompt;
- image analysis;
- video analysis;
- tool calling.

`oxide-agent-core/src/llm/providers/mod.rs` imports and re-exports all provider implementations unconditionally:

- ChatGPT/OpenAI-style provider;
- Groq provider;
- Minimax provider;
- Mistral provider;
- Nvidia provider;
- OpenCode Go provider;
- OpenRouter provider;
- ZAI provider.

`oxide-agent-core/src/llm/client.rs` builds a `HashMap<String, Arc<dyn LlmProvider>>` from global settings. Provider inclusion is determined by runtime config and credential presence, not Cargo features.

Observed runtime provider insertion behavior:

- ChatGPT provider is inserted if an auth path exists.
- Groq provider is inserted if a Groq API key exists.
- Mistral provider is inserted if a Mistral API key exists.
- Minimax provider is inserted if a Minimax API key exists.
- ZAI provider is inserted if a ZAI API key exists.
- Nvidia provider is inserted if a Nvidia API key exists.
- OpenCode Go provider is inserted if an OpenCode Go API key exists.
- OpenCode Go aliases are registered globally.
- OpenRouter provider is inserted if an OpenRouter API key exists.

Embedding provider creation is also a centralized hardcoded match. It switches over provider strings such as Google, Mistral, OpenRouter, and OpenAI-compatible provider names.

Global model capability and alias logic currently lives in shared code rather than provider modules.

Current LLM provider problems:

- Provider modules and SDK dependencies are compiled even when unused.
- There is no per-provider Cargo feature boundary.
- Provider aliases are declared centrally rather than by provider modules.
- Model capability routing is globally hardcoded.
- Embedding provider creation is globally hardcoded.
- Config validation checks runtime credentials but not compiled provider availability.
- Runtime config determines availability even though dependency elimination requires compile-time gating.

Target state:

- Each LLM provider must be an independent capability module.
- Each provider must own its aliases, config schema, default model metadata, and factory.
- No provider-specific dependency may compile unless the provider feature is enabled.
- Model routing must reference provider IDs from the compiled capability manifest.
- Config for a non-compiled provider must fail loudly.
- Direct Google Gemini is no longer a target provider. Gemini-family models must be accessed through OpenRouter routes; the former direct provider feature, provider alias, direct credentials, and vendored SDK code must stay absent.

### 4.5 Current storage architecture

The current storage module is conceptually trait-based but operationally R2-first.

`oxide-agent-core/src/storage/provider.rs` defines a broad `StorageProvider` trait for:

- user config;
- chat history;
- memory;
- control plane data;
- reminders;
- wiki memory;
- audits;
- agent profiles;
- manager topic state;
- sandbox/topic metadata.

`oxide-agent-core/src/storage/r2.rs`, `r2_base.rs`, and `r2_provider.rs` implement R2 storage using AWS SDK S3 client types. The storage module comments and key layout assume Cloudflare R2 / AWS S3 semantics.

`oxide-agent-transport-telegram/src/runner.rs` hardcodes storage startup through `R2Storage::new(settings)`. It exits startup if R2 initialization fails. It then passes the R2 instance as `Arc<dyn StorageProvider>` into later runtime code.

Startup maintenance also accepts `Arc<R2Storage>` directly, not `Arc<dyn StorageProvider>`, for persisted tool drift cleanup.

Current storage problems:

- R2/AWS SDK dependencies are unconditional in `oxide-agent-core`.
- Local/embedded builds cannot eliminate AWS SDK.
- Production startup is hardcoded to R2.
- The storage abstraction is too broad and not module-oriented.
- Startup maintenance depends on the concrete R2 type.
- Earlier minimal-build examples depended on a local durable backend, but the later storage decision rejects durable local storage.
- Storage backend selection is not part of the module registry.

Target state:

- `storage-s3-r2` must be the only durable storage backend.
- `storage-local-fs` may exist only for transient runtime workspace data, not durable state.
- Storage backends must register through the unified module registry.
- Tools and transports must consume storage interfaces, not concrete R2 types.
- AWS SDK must be absent unless `storage-s3-r2` is selected.

### 4.6 Current sandbox architecture

The current sandbox architecture mixes backend selection, file operations, command execution, file delivery, stack logs, and Docker-specific details.

`oxide-agent-core/src/sandbox/manager.rs` uses a `SandboxManager` wrapper with two runtime-selected modes:

- direct Docker backend via Bollard;
- broker backend via `sandboxd` Unix socket.

Selection is controlled by `SANDBOX_BACKEND=broker`; otherwise Docker direct is used.

Direct Docker implementation currently compiles unconditionally and includes:

- container creation;
- command execution;
- file upload/download;
- file read/write;
- sandbox recreation;
- size inspection;
- compose stack log access;
- prune/list/inspect operations;
- hardcoded resource defaults;
- hardcoded image defaults;
- Docker socket assumptions.

`oxide-agent-core/src/sandbox/broker.rs` defines a bincode/Unix-socket protocol for sandbox daemon requests.

`oxide-agent-sandboxd` starts the broker server and also performs SSH temp-file cleanup.

The current sandbox tool provider exposes all of these capabilities together:

- command execution;
- file writing;
- file reading;
- file delivery to user;
- file listing;
- sandbox recreation.

The `execute_command` tool implies a heavy sandbox environment containing Python, ffmpeg, yt-dlp, networking tools, and other utilities. That environment is not appropriate for lite or embedded builds unless explicitly selected.

Current sandbox problems:

- Bollard/Docker dependencies are unconditional.
- Direct Docker and sandbox daemon client code are runtime-selected rather than compile-time selected.
- Command execution and file operations are bundled into one tool provider.
- File delivery is bundled with sandbox operations.
- Stack logs are coupled to Docker Compose assumptions.
- The sandbox image is fat and universal.
- `execute_command` can leak into profiles that only need file read/write.
- SSH cleanup is performed by main binaries outside an SSH module.

Target state:

- Sandbox backend modules must be separate from sandbox tool modules.
- File operations must be separate from command execution.
- `execute_command` must not be present in embedded/lite profiles unless explicitly enabled.
- Docker direct and sandboxd broker must be separate backend modules.
- Sandbox image variants must be generated from or selected by module requirements.

### 4.7 Current feature flags and dependency graph

The current `oxide-agent-core` feature layout is too coarse:

```toml
[features]
default = ["tavily", "searxng"]
tavily = ["dep:tavily"]
searxng = []
browser_use = []
jira = []
mattermost = []
```

Most heavy dependencies are not optional in the current core crate.

Unconditional dependency groups include:

- AWS SDK / S3 storage dependencies;
- Bollard Docker client;
- RMCP client/child-process support;
- OpenAI-compatible SDK;
- ZAI SDK;
- HTTP client with multipart/stream/cookie features;
- tokenization and model support dependencies;
- caching and persistence support;
- cron/timezone scheduling;
- tar/bincode/serialization dependencies used by sandbox and broker features;
- media-adjacent and provider-adjacent support libraries.

The existing optional features mainly control registration/runtime integration, not dependency elimination.

Current feature problems:

- `default` enables search-related features.
- There is no `default = []` minimal build.
- Cargo features do not correspond to atomic capabilities.
- Provider-specific dependencies are compiled even when provider credentials are absent.
- Storage and sandbox dependencies are compiled even when no storage/sandbox profile needs them.
- Dockerfile builds force selected full-profile features.

Target state:

- `default = []`.
- Atomic features for every provider/tool/backend/integration.
- Profile features composed from atomic features.
- CI verifies dependency absence for selected profiles.

### 4.8 Current architectural problems

The current architecture has these concrete problems:

- Runtime disabling does not remove dependencies from the binary.
- Providers are compiled even when unused.
- LLM provider registration is controlled by env/config rather than compiled module availability.
- Storage startup is hardcoded to R2.
- AWS SDK cannot be eliminated from builds that do not select `storage-s3-r2`.
- Docker sandbox support and Bollard cannot be eliminated from no-sandbox builds.
- Tools are registered through duplicated legacy and typed runtime registries.
- Legacy providers are wrapped into typed executors instead of being replaced by first-class modules.
- Broad providers mix unrelated capabilities, especially sandbox and manager tools.
- Docker profile does not define architecture; it ships a full runtime image.
- Compose always starts sidecars that many profiles do not need.
- Sandbox image is fat and universal.
- MCP binaries are copied into images even when integrations are not selected.
- Browser/search/media capabilities are not cleanly separated.
- Transport startup performs application composition and storage initialization.
- Some cleanup/migration/compatibility behavior is embedded in startup paths.
- Config uses global fields and old env conventions rather than module-specific schemas.
- Capability boundaries are unclear.

## 5. Target Architecture

The target architecture is a Capability Module System.

A capability module is the first-class unit of compilation, registration, configuration, validation, runtime availability, Docker requirements, and Compose requirements.

Capability module kinds:

- tool modules;
- LLM provider modules;
- storage backend modules;
- sandbox backend modules;
- sandbox tool modules;
- transport modules;
- MCP integration modules;
- search modules;
- browser modules;
- media modules;
- memory/wiki modules;
- reminder/task modules;
- file delivery modules;
- diagnostics/manager modules.

Every module must declare:

- stable unique module ID;
- module kind;
- Cargo feature name;
- optional dependencies;
- registration function or factory;
- config schema;
- runtime validation hook if needed;
- exported capabilities;
- required capabilities;
- conflicting capabilities if any;
- required external services if any;
- required Docker runtime assets if any;
- required Compose services if any;
- health check if it owns an external dependency.

The registry must be assembled from compiled modules. Runtime config can enable or disable modules that exist in the compiled module list. Runtime config cannot make unavailable modules appear.

High-level target flow:

1. Binary starts.
2. Compiled modules expose a static module list through feature-gated inventory functions.
3. Config is loaded.
4. Config is validated against the compiled capability manifest.
5. The `RuntimeContext` is built from selected modules.
6. Each enabled module registers itself into the unified registry.
7. The selected transport starts using the `RuntimeContext`.

The architecture must prefer explicit composition over global runtime discovery.

## 6. Capability Model

A capability is a typed, addressable unit of behavior exposed by a module.

Capability ID format:

```text
<kind>/<name>
```

Examples:

```text
llm-provider/opencode-go
llm-provider/openai-chatgpt
llm-provider/groq
llm-provider/mistral
llm-provider/minimax
llm-provider/zai
llm-provider/nvidia
llm-provider/openrouter

tool/tavily-search
tool/tavily-extract
tool/webfetch-md
tool/searxng-search
tool/browser-use
tool/todos
tool/agents-md
tool/reminder
tool/wiki-memory
tool/compression
tool/delegation
tool/file-delivery
tool/sandbox-fileops
tool/sandbox-exec
tool/sandbox-recreate
tool/sandbox-list-files
tool/stack-logs
tool/media-audio-transcription
tool/media-image-description
tool/media-video-description
tool/ytdlp-metadata
tool/ytdlp-transcript
tool/ytdlp-download
tool/tts-kokoro
tool/tts-silero

storage/local
storage/r2

transport/telegram
transport/web
transport/cli
transport/http-api

sandbox-backend/docker-direct
sandbox-backend/sandboxd-client
sandbox-daemon/sandboxd

integration/mcp-jira
integration/mcp-mattermost
integration/ssh-mcp

manager/control-plane
manager/topic-sandbox-admin
manager/agent-profile-admin
```

Each capability must be independently includable when technically possible. If a capability depends on another capability, the dependency must be declared explicitly.

Examples:

- `tool/sandbox-fileops` requires one sandbox backend capability.
- `tool/sandbox-exec` requires one sandbox backend with `exec` support.
- `tool/file-delivery` requires a file delivery sink from the active transport or a storage-backed file hoster.
- `tool/media-audio-transcription` requires an LLM provider that declares audio transcription support.
- `tool/media-video-description` requires an LLM provider or media processor that declares video support.
- `integration/mcp-jira` requires the RMCP client and the Jira MCP binary/service.
- `tool/searxng-search` requires the searxng service URL and a Compose service in Docker deployments unless an external URL is configured.
- `tool/browser-use` requires the browser-use bridge service unless configured to use an external bridge.

Capability dependency declarations must be machine-readable. They must drive config validation, registry construction, generated manifests, Docker assets, and Compose service requirements.

## 7. Cargo Feature Architecture

Cargo features must be redesigned around atomic modules and profile compositions.

### 7.1 Rules

- `default = []`.
- Every provider must have its own feature.
- Every tool module must have its own feature or small atomic feature group.
- Every storage backend must have its own feature.
- Every sandbox backend must have its own feature.
- Every transport must have its own feature.
- Every MCP integration must have its own feature.
- Every heavy dependency must be `optional = true`.
- Profile features must be compositions of atomic features.
- Runtime config must not enable capabilities that are absent from the compiled feature set.
- Feature names must be stable, lowercase, kebab-case, and prefixed by module kind.

### 7.2 Proposed feature naming convention

```toml
[features]
default = []

# Profile features
profile-full = [
  "transport-telegram",
  "transport-web",
  "storage-s3-r2",
  "llm-chatgpt",
  "llm-groq",
  "llm-mistral",
  "llm-minimax",
  "llm-zai",
  "llm-nvidia",
  "llm-opencode-go",
  "llm-openrouter",
  "tool-todos",
  "tool-compression",
  "tool-delegation",
  "tool-agents-md",
  "tool-reminder",
  "tool-wiki-memory",
  "tool-webfetch-md",
  "tool-tavily",
  "tool-searxng",
  "tool-browser-use",
  "tool-sandbox-fileops",
  "tool-sandbox-exec",
  "tool-sandbox-recreate",
  "tool-file-delivery",
  "tool-media-audio",
  "tool-media-image",
  "tool-media-video",
  "tool-ytdlp",
  "tool-tts-kokoro",
  "tool-tts-silero",
  "tool-stack-logs",
  "sandbox-backend-docker-direct",
  "sandbox-backend-sandboxd-client",
  "sandbox-daemon",
  "integration-mcp-jira",
  "integration-mcp-mattermost",
  "integration-ssh-mcp",
  "manager-control-plane",
]

profile-embedded-opencode-local = [
  "transport-telegram",
  "storage-s3-r2",
  "llm-opencode-go",
  "tool-todos",
  "tool-agents-md",
  "tool-reminder",
  "tool-wiki-memory",
  "tool-webfetch-md",
  "tool-tavily",
  "tool-sandbox-fileops",
  "sandbox-backend-sandboxd-client",
]

profile-lite = [
  "transport-telegram",
  "storage-s3-r2",
  "llm-opencode-go",
  "tool-todos",
  "tool-webfetch-md",
  "tool-reminder",
]

profile-search-only = [
  "transport-telegram",
  "storage-s3-r2",
  "llm-opencode-go",
  "tool-webfetch-md",
  "tool-tavily",
]

profile-no-sandbox = [
  "transport-telegram",
  "storage-s3-r2",
  "llm-opencode-go",
  "tool-todos",
  "tool-webfetch-md",
  "tool-reminder",
  "tool-wiki-memory",
]

profile-media-enabled = [
  "transport-telegram",
  "storage-s3-r2",
  "llm-opencode-go",
  "tool-media-audio",
  "tool-media-image",
  "tool-media-video",
  "tool-file-delivery",
]

# Transports
transport-telegram = ["dep:teloxide"]
transport-web = ["dep:axum"]
transport-cli = []
transport-http-api = ["dep:axum"]

# Storage
storage-s3-r2 = ["dep:aws-sdk-s3", "dep:aws-config", "dep:aws-credential-types", "dep:aws-types"]
storage-local-fs = []

# LLM providers
llm-chatgpt = ["dep:async-openai"]
llm-groq = []
llm-mistral = []
llm-minimax = []
llm-zai = ["dep:zai-rs"]
llm-nvidia = []
llm-opencode-go = []
llm-openrouter = []

# Search and browser tools
tool-webfetch-md = ["dep:reqwest", "dep:htmd"]
tool-tavily = ["dep:tavily"]
tool-searxng = ["dep:reqwest"]
tool-browser-use = ["dep:reqwest"]

# Sandbox
tool-sandbox-fileops = []
tool-sandbox-exec = []
tool-sandbox-recreate = []
tool-file-delivery = []
tool-stack-logs = []
sandbox-backend-docker-direct = ["dep:bollard", "dep:tar"]
sandbox-backend-sandboxd-client = ["dep:bincode", "dep:serde_bytes"]
sandbox-daemon = ["sandbox-backend-docker-direct", "dep:bincode", "dep:serde_bytes"]

# MCP and SSH integrations
integration-mcp-jira = ["dep:rmcp"]
integration-mcp-mattermost = ["dep:rmcp"]
integration-ssh-mcp = ["dep:rmcp"]
```

The exact dependency mappings must be finalized during Milestone 1, but the naming model and atomic structure are mandatory.

### 7.3 Binary feature requirements

Binaries must declare `required-features` where appropriate.

Examples:

```toml
[[bin]]
name = "oxide-agent"
path = "src/bin/oxide-agent.rs"
required-features = ["transport-telegram"]

[[bin]]
name = "oxide-agent-sandboxd"
path = "src/bin/oxide-agent-sandboxd.rs"
required-features = ["sandbox-daemon"]
```

A build that does not enable `sandbox-daemon` must not build or ship `oxide-agent-sandboxd`.

## 8. Unified Module Registry

The new architecture must have exactly one module registry and exactly one module registration path.

### 8.1 Registry requirements

- One registry builder.
- One source of truth for compiled modules.
- No legacy registry.
- No separate typed registry that re-registers the same tools.
- No adapter that wraps legacy providers into typed executors.
- Each module registers its own tools/providers/backends/transports.
- Registry construction starts from compile-time available modules.
- Runtime config may disable compiled modules.
- Runtime config cannot enable absent modules.
- Registry must expose both compiled and enabled capabilities.
- Duplicate IDs must be hard errors.
- Capability dependency failures must be hard errors.

### 8.2 Core traits

```rust
pub trait CapabilityModule: Send + Sync {
    fn id(&self) -> ModuleId;
    fn kind(&self) -> CapabilityKind;
    fn cargo_feature(&self) -> &'static str;

    fn provides(&self) -> &'static [CapabilityId];
    fn requires(&self) -> &'static [CapabilityRequirement];
    fn conflicts(&self) -> &'static [CapabilityId] { &[] }

    fn config_schema(&self) -> ModuleConfigSchema;

    fn docker_requirements(&self) -> &'static [DockerRequirement] { &[] }
    fn compose_requirements(&self) -> &'static [ComposeRequirement] { &[] }

    fn validate_config(&self, ctx: &ValidationContext, config: &ModuleConfig) -> Result<()>;

    fn register(&self, ctx: &ModuleContext, registry: &mut ModuleRegistry) -> Result<()>;
}
```

```rust
pub struct ModuleRegistry {
    pub manifest: CompiledCapabilityManifest,
    pub enabled: EnabledCapabilityManifest,
    pub tools: ToolRegistry,
    pub llm_providers: LlmProviderRegistry,
    pub storage_backends: StorageBackendRegistry,
    pub sandbox_backends: SandboxBackendRegistry,
    pub transports: TransportRegistry,
    pub health_checks: HealthCheckRegistry,
}
```

```rust
pub struct ModuleContext {
    pub app: Arc<AppContext>,
    pub config: Arc<ResolvedConfig>,
    pub storage: Option<Arc<dyn StorageProvider>>,
    pub sandbox: Option<Arc<dyn SandboxBackend>>,
    pub llm: Arc<LlmRouter>,
    pub events: Arc<EventBus>,
}
```

### 8.3 Compiled module list

The compiled module list must be generated through feature-gated functions.

```rust
pub fn compiled_modules() -> Vec<Box<dyn CapabilityModule>> {
    let mut modules: Vec<Box<dyn CapabilityModule>> = Vec::new();

    #[cfg(feature = "llm-opencode-go")]
    modules.push(Box::new(OpencodeGoLlmModule));

    #[cfg(feature = "storage-s3-r2")]
    modules.push(Box::new(S3R2StorageModule));

    #[cfg(feature = "tool-webfetch-md")]
    modules.push(Box::new(WebFetchMdToolModule));

    #[cfg(feature = "tool-sandbox-fileops")]
    modules.push(Box::new(SandboxFileOpsModule));

    modules
}
```

A macro or inventory crate may be used only if it preserves deterministic output and compile-time feature gating. The generated manifest must be deterministic for snapshot tests.

### 8.4 Registry build flow

```rust
pub fn build_runtime(config: RawConfig) -> Result<RuntimeContext> {
    let modules = compiled_modules();
    let compiled_manifest = CompiledCapabilityManifest::from_modules(&modules)?;

    let resolved_config = ConfigResolver::new(compiled_manifest.clone())
        .resolve(config)?
        .validate()?;

    let enabled_modules = ModuleSelector::new(&compiled_manifest, &resolved_config)
        .enabled_modules(&modules)?;

    let mut registry = ModuleRegistry::new(compiled_manifest, enabled_modules.manifest());
    let app_ctx = AppContextBuilder::new(&resolved_config, &registry).build_base()?;
    let module_ctx = ModuleContext::new(app_ctx, resolved_config);

    for module in enabled_modules {
        module.register(&module_ctx, &mut registry)?;
    }

    registry.validate_dependencies()?;
    RuntimeContext::from_registry(registry)
}
```

### 8.5 Manifest output

Every binary must be able to print compiled capabilities for debugging and CI:

```bash
oxide-agent capabilities --compiled
oxide-agent capabilities --enabled --config config/embedded.yml
oxide-agent capabilities --json
```

This output is required for snapshot tests and Docker/Compose generation.

## 9. Tool Architecture

Tool modules must be atomic and self-contained. A tool definition and executor must live together.

### 9.1 Tool module trait

```rust
pub trait ToolModule: CapabilityModule {
    fn tools(&self) -> &'static [ToolSpec];

    fn register_tools(
        &self,
        ctx: &ModuleContext,
        registry: &mut ToolRegistry,
    ) -> Result<()>;
}
```

```rust
pub trait ToolExecutor: Send + Sync {
    fn name(&self) -> ToolName;
    fn spec(&self) -> ToolSpec;
    fn required_capabilities(&self) -> &'static [CapabilityId];

    async fn execute(&self, invocation: ToolInvocation, ctx: ToolExecutionContext) -> Result<ToolOutput>;
}
```

The new `ToolRegistry` must be typed. It must not support legacy provider dispatch.

```rust
pub struct ToolRegistry {
    executors: BTreeMap<ToolName, Arc<dyn ToolExecutor>>,
}
```

Duplicate tool names must fail registry construction.

### 9.2 Required tool decomposition

The current broad providers must be split.

#### Sandbox tools

Replace the current broad `SandboxToolProvider` with separate modules:

- `SandboxFileOpsModule`
  - `write_file`
  - `read_file`
  - optionally `list_files` if it only requires file operations;

- `SandboxExecModule`
  - `execute_command` only;
  - requires an exec-capable sandbox backend;
  - never enabled in lite/embedded profiles unless explicitly selected;

- `SandboxRecreateModule`
  - `recreate_sandbox`;
  - requires backend lifecycle management;

- `FileDeliveryModule`
  - `send_file_to_user`;
  - depends on transport/file delivery sink and optional storage backend;

- `StackLogsModule`
  - stack log tools;
  - depends on Docker/Compose-aware sandbox diagnostics.

#### Search and fetch tools

- `WebFetchMdModule`
  - `web_markdown`;
  - no browser dependency;
  - no searxng dependency.

- `TavilySearchModule`
  - `web_search`;
  - `web_extract`;
  - requires Tavily API key/config;
  - compiles Tavily SDK only when enabled.

- `SearxngSearchModule`
  - `searxng_search`;
  - requires service URL;
  - declares Compose requirement if not using external URL.

- `BrowserUseModule`
  - browser task/session/content/screenshot tools;
  - requires browser-use bridge service or external URL;
  - no search dependency unless explicitly combined by a profile.

#### Media tools

Split media by capability:

- `MediaAudioTranscriptionModule`
  - `transcribe_audio_file`;
  - requires audio-capable LLM provider or media processor.

- `MediaImageDescriptionModule`
  - `describe_image_file`;
  - requires image-capable LLM provider.

- `MediaVideoDescriptionModule`
  - `describe_video_file`;
  - requires video-capable LLM provider or video extraction pipeline.

#### Yt-dlp tools

Yt-dlp support must be separate from generic media tools:

- `YtdlpMetadataModule`;
- `YtdlpTranscriptModule`;
- `YtdlpDownloadModule`.

These modules must declare sandbox image package requirements if they need `yt-dlp`, Python, ffmpeg, or network tooling.

#### MCP tools

MCP tools must be isolated modules:

- `McpJiraModule`;
- `McpMattermostModule`;
- `SshMcpModule`.

Each module must own:

- feature gate;
- RMCP dependency;
- child process binary requirement or service requirement;
- config schema;
- tool definitions;
- health validation;
- cleanup behavior if any.

SSH cleanup must move into `SshMcpModule`. Main binaries and sandbox daemon must not run SSH cleanup unconditionally.

#### Core utility tools

These modules should remain lightweight and independent:

- `TodosModule`;
- `CompressionModule`;
- `AgentsMdModule`;
- `ReminderModule`;
- `WikiMemoryModule`;
- `DelegationModule`.

`DelegationModule` must explicitly declare which sandbox/file/memory capabilities it requires. It must not assume all sandbox tools exist.

#### Manager/control-plane tools

Manager tools must be split into capability groups:

- `ManagerForumTopicModule`;
- `ManagerTopicBindingModule`;
- `ManagerTopicContextModule`;
- `ManagerTopicAgentsMdModule`;
- `ManagerTopicInfraModule`;
- `ManagerTopicSandboxAdminModule`;
- `ManagerAgentProfileModule`;
- `ManagerToolPolicyModule`;
- `ManagerHookPolicyModule`;
- `ManagerPrivateSecretProbeModule`.

A profile may include the whole `manager-control-plane` composition feature, but the implementation must keep submodules internally separated.

### 9.3 Tool dependencies

Every tool spec must include dependency declarations.

```rust
pub struct ToolSpec {
    pub name: ToolName,
    pub description: &'static str,
    pub input_schema: JsonSchema,
    pub output_schema: Option<JsonSchema>,
    pub requires: &'static [CapabilityId],
    pub risk: ToolRiskLevel,
}
```

Example:

```rust
ToolSpec {
    name: ToolName::new("execute_command"),
    requires: &[cap!("sandbox/exec"), cap!("sandbox-backend/exec-capable")],
    risk: ToolRiskLevel::High,
    ..
}
```

### 9.4 Tool runtime policy

Runtime tool policy may hide or disable compiled tools for a specific topic/session/model. It must not be used as a substitute for compile-time modularity.

Policy layers:

1. compile-time capability availability;
2. config-enabled modules;
3. transport/session/topic policy;
4. model-specific tool compatibility.

A tool must pass all layers to appear in the final tool list.

## 10. LLM Provider Architecture

Each LLM provider must be a module with a feature gate, config schema, aliases, factory, and declared model capabilities.

### 10.1 Provider module trait

```rust
pub trait LlmProviderModule: CapabilityModule {
    fn provider_id(&self) -> ProviderId;
    fn aliases(&self) -> &'static [&'static str];
    fn supported_model_capabilities(&self) -> &'static [ModelCapability];

    fn build_provider(
        &self,
        config: &ProviderConfig,
        ctx: &ProviderBuildContext,
    ) -> Result<Arc<dyn LlmProvider>>;
}
```

```rust
pub struct LlmProviderRegistry {
    providers: BTreeMap<ProviderId, Arc<dyn LlmProviderFactory>>,
    aliases: BTreeMap<String, ProviderId>,
}
```

### 10.2 Required provider modules

Create provider modules for the currently compiled providers:

- `ChatGptProviderModule` / `llm-chatgpt`;
- `GroqProviderModule` / `llm-groq`;
- `MistralProviderModule` / `llm-mistral`;
- `MinimaxProviderModule` / `llm-minimax`;
- `ZaiProviderModule` / `llm-zai`;
- `NvidiaProviderModule` / `llm-nvidia`;
- `OpencodeGoProviderModule` / `llm-opencode-go`;
- `OpenRouterProviderModule` / `llm-openrouter`.

Each provider module must own:

- provider ID;
- aliases;
- API endpoint defaults;
- credential config fields;
- model capability metadata;
- chat/tool-call/media support declaration;
- factory implementation;
- health validation behavior.

### 10.3 Provider routing

Model routing must reference provider IDs, not global string heuristics.

Example config:

```yaml
routes:
  chat:
    provider: llm-provider/opencode-go
    model: openai/gpt-oss-120b

  media_image:
    provider: llm-provider/openrouter
    model: google/gemini-2.5-flash

```

If a removed/non-compiled direct Google Gemini provider is configured, startup must fail. Gemini-family model IDs remain valid only as OpenRouter model IDs.

### 10.4 Provider aliases

Aliases must be declared by provider modules.

Example:

```rust
impl LlmProviderModule for OpencodeGoProviderModule {
    fn provider_id(&self) -> ProviderId {
        ProviderId::new("llm-provider/opencode-go")
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["opencode-go", "opencode_go"]
    }
}
```

Global alias lists must be removed.

### 10.5 Embeddings

Embeddings are not part of the target architecture.

The current embedding code exists only to support the legacy skills subsystem. Since skills are removed, embedding providers, embedding config, embedding cache, and embedding model routing must also be removed.

Future retrieval must not reintroduce embeddings by default. If semantic retrieval is ever required later, it must be proposed as a new explicit capability with S3/R2-backed durable indexes and separate acceptance criteria. It is out of scope for this refactor.

### 10.6 LLM client target shape

`LlmClient` should become a router over registered provider factories and configured routes.

```rust
pub struct LlmRouter {
    providers: BTreeMap<ProviderId, Arc<dyn LlmProvider>>,
    routes: ModelRoutes,
}
```

`LlmRouter` must be built from enabled provider modules. It must not import all provider implementations directly.

## 11. Storage Architecture

Storage must become a modular subsystem with backend-specific modules and smaller service traits.

### 11.1 Required backend modules

- `S3R2StorageModule` / `storage-s3-r2`.

Future backends must be addable without editing transport startup:

- Postgres;
- memory-only test storage;

Future durable local filesystem or SQLite storage is out of scope and would require a new PRD decision that supersedes section 22.7.

### 11.2 Storage backend trait

The current giant `StorageProvider` can remain temporarily as an internal aggregate only if it is implemented by backend modules and not by startup code directly. The target should split it into smaller traits.

Recommended target traits:

```rust
pub trait StorageProvider: Send + Sync {
    fn id(&self) -> StorageBackendId;
    fn capabilities(&self) -> &'static [StorageCapability];
}

pub trait ConversationStore: Send + Sync { /* chat history */ }
pub trait UserConfigStore: Send + Sync { /* user config */ }
pub trait MemoryStore: Send + Sync { /* memory */ }
pub trait ReminderStore: Send + Sync { /* reminders */ }
pub trait WikiStore: Send + Sync { /* wiki memory */ }
pub trait ControlPlaneStore: Send + Sync { /* manager/control-plane */ }
pub trait AuditStore: Send + Sync { /* audit records */ }
pub trait FileBlobStore: Send + Sync { /* optional file/object storage */ }
```

A backend module may provide multiple storage traits.

### 11.3 Transient local filesystem workspace

`storage-local-fs` is not a durable storage backend. It may provide only transient runtime workspace paths for temporary files, sandbox workdirs, download buffers, upload staging, and disposable cache. It must not be accepted as the source of truth for conversations, todos, reminders, wiki/memory data, artifacts, generated files, manifests, or any other durable agent state.

Rules:

- no durable agent state may depend on local filesystem persistence;
- persistent startup must fail loudly if `storage-s3-r2` configuration is missing or invalid;
- tests may use memory/noop storage helpers, but production profiles must use `storage-s3-r2`.

### 11.4 S3/R2 storage

`storage-s3-r2` must own all AWS SDK dependencies and S3/R2-specific config. The runtime module ID may remain `storage/r2` while Cloudflare R2 is the configured S3-compatible target, but the Cargo feature and architecture label are `storage-s3-r2`.

R2 module config:

```yaml
modules:
  storage/r2:
    enabled: true
    endpoint: https://...
    bucket: oxide-agent
    region: auto
    credentials:
      access_key_id: ${OXIDE_R2_ACCESS_KEY_ID}
      secret_access_key: ${OXIDE_R2_SECRET_ACCESS_KEY}
```

No R2 fields should live in global app settings after the refactor.

### 11.5 Startup ownership

Transport runners must not instantiate `R2Storage` or any concrete backend. App startup must:

1. select exactly one primary storage backend unless a profile explicitly supports multiple;
2. build it through the selected storage module;
3. inject storage trait objects into `RuntimeContext`.

Startup maintenance must not accept concrete R2 types. Migration-oriented cleanup paths must be deleted unless they are reintroduced as explicit optional maintenance modules.

## 12. Sandbox Architecture

Sandbox must be modularized into backend modules and tool modules.

### 12.1 Backend modules

Required backend modules:

- `DockerDirectSandboxBackendModule` / `sandbox-backend-docker-direct`;
- `SandboxdClientBackendModule` / `sandbox-backend-sandboxd-client`;
- `SandboxDaemonModule` / `sandbox-daemon`.

The daemon is a deployable binary/service. The client backend is what the main agent uses to talk to the daemon.

### 12.2 Backend traits

```rust
pub trait SandboxBackend: Send + Sync {
    fn id(&self) -> SandboxBackendId;
    fn capabilities(&self) -> &'static [SandboxCapability];
}

pub trait SandboxFileOps: SandboxBackend {
    async fn write_file(&self, scope: SandboxScope, path: &str, bytes: Bytes) -> Result<()>;
    async fn read_file(&self, scope: SandboxScope, path: &str) -> Result<Bytes>;
    async fn list_files(&self, scope: SandboxScope, path: &str) -> Result<Vec<FileEntry>>;
}

pub trait SandboxExec: SandboxBackend {
    async fn exec(&self, scope: SandboxScope, command: ExecCommand) -> Result<ExecOutput>;
}

pub trait SandboxLifecycle: SandboxBackend {
    async fn recreate(&self, scope: SandboxScope) -> Result<()>;
}

pub trait SandboxDiagnostics: SandboxBackend {
    async fn stack_logs(&self, query: StackLogQuery) -> Result<StackLogOutput>;
}
```

Tool modules must require the smallest trait they need.

### 12.3 Tool split

- `tool-sandbox-fileops`
  - requires `SandboxFileOps`;
  - provides `write_file`, `read_file`, optionally `list_files`.

- `tool-sandbox-exec`
  - requires `SandboxExec`;
  - provides `execute_command`;
  - high-risk;
  - disabled by default in embedded/lite profiles.

- `tool-sandbox-recreate`
  - requires `SandboxLifecycle`;
  - provides `recreate_sandbox`.

- `tool-stack-logs`
  - requires `SandboxDiagnostics`;
  - should not be part of minimal sandbox.

- `tool-file-delivery`
  - separate from sandbox backend;
  - can read from sandbox through `SandboxFileOps` but sends through transport/file service.

### 12.4 Sandbox image variants

The fat universal image must be replaced by profile-specific variants.

Required variants:

- `sandbox-minimal`
  - shell, coreutils, CA certificates, curl if required by selected tools;
  - no Python, ffmpeg, yt-dlp, nmap, mtr, browser packages unless selected.

- `sandbox-exec`
  - adds packages required for command execution profiles.

- `sandbox-media`
  - adds ffmpeg, yt-dlp, Python packages, and media utilities.

- `sandbox-dev`
  - full diagnostic image for development.

Modules must declare sandbox package requirements.

Example:

```rust
DockerRequirement::SandboxPackages(&["python3", "python3-pip"])
DockerRequirement::SandboxPackages(&["ffmpeg"])
DockerRequirement::SandboxPackages(&["yt-dlp"])
```

The sandbox image selection/generation process must merge selected requirements for the active profile.

### 12.5 Security posture

`execute_command` must not be included by implication. Profiles that include it must state so explicitly.

`embedded-opencode-local` must not include `tool-sandbox-exec` unless the profile is deliberately changed.

## 13. Transport Architecture

Transports must be modules. A transport must not hardcode storage, LLM providers, sandbox backends, or sidecars.

Required transport modules:

- `TelegramTransportModule` / `transport-telegram`;
- `WebTransportModule` / `transport-web`;
- `CliTransportModule` / `transport-cli`;
- `HttpApiTransportModule` / `transport-http-api`.

A transport module owns:

- transport-specific config schema;
- startup/serve implementation;
- file delivery sink if supported;
- progress/event rendering;
- authentication/session mapping;
- health checks.

A transport consumes `RuntimeContext`:

```rust
pub trait TransportModule: CapabilityModule {
    async fn run(&self, ctx: RuntimeContext) -> Result<()>;
}
```

`RuntimeContext` provides:

- tool registry;
- LLM router;
- storage interfaces;
- sandbox interfaces if enabled;
- reminder scheduler if enabled;
- event bus;
- configured policies.

Telegram-specific startup must stop doing these tasks directly:

- constructing R2 storage;
- constructing all LLM providers;
- deciding sandbox backend;
- running migration/drift cleanup;
- deciding which MCP integrations exist;
- acting as the application composition root.

The composition root must move into a profile-aware app bootstrap layer.

## 14. Config Architecture

Config must be module-driven and validated against compiled capabilities.

### 14.1 Config rules

- Config is loaded after the compiled capability manifest is known.
- Unknown module IDs fail startup.
- Config for a non-compiled module fails startup.
- Config for a compiled but disabled module is allowed only under that module key with `enabled: false`.
- Runtime-disabled modules cannot register tools/providers/backends.
- Module dependencies must be validated before startup.
- Exactly one primary transport must be selected unless the binary supports multi-transport mode.
- Exactly one primary storage backend must be selected unless multi-storage mode is explicit.
- Provider routes must reference compiled and enabled provider IDs.
- Old env compatibility is not required.

### 14.2 Proposed config shape

```yaml
profile: embedded-opencode-local

modules:
  transport/telegram:
    enabled: true
    bot_token: ${OXIDE_TELEGRAM_BOT_TOKEN}
    manager_chat_id: ${OXIDE_TELEGRAM_MANAGER_CHAT_ID}

  storage/local:
    enabled: true
    path: /data/oxide-agent

  llm-provider/opencode-go:
    enabled: true
    base_url: http://opencode-go:8080/v1
    api_key: ${OXIDE_OPENCODE_GO_API_KEY}

  tool/webfetch-md:
    enabled: true

  tool/tavily-search:
    enabled: true
    api_key: ${OXIDE_TAVILY_API_KEY}

  tool/sandbox-fileops:
    enabled: true

  tool/sandbox-exec:
    enabled: false

routes:
  chat:
    provider: llm-provider/opencode-go
    model: openai/gpt-oss-120b
```

### 14.3 Env mapping

Environment variables may remain supported, but they must map into module config rather than global settings fields.

Recommended convention:

```text
OXIDE_MODULE__LLM_PROVIDER_OPENCODE_GO__API_KEY
OXIDE_MODULE__LLM_PROVIDER_OPENCODE_GO__BASE_URL
OXIDE_MODULE__STORAGE_LOCAL__PATH
OXIDE_MODULE__TRANSPORT_TELEGRAM__BOT_TOKEN
OXIDE_MODULE__TOOL_TAVILY_SEARCH__API_KEY
```

The exact convention may be simplified, but it must be deterministic and module-scoped.

### 14.4 Config schema

Each module must provide a schema object. The app must be able to emit a config schema for compiled modules:

```bash
oxide-agent config schema --compiled --json
oxide-agent config example --profile embedded-opencode-local
```

### 14.5 Removed config behavior

Delete:

- old env aliases;
- deprecated fields;
- temporary migration switches;
- global R2 fields;
- global provider key fields;
- global sandbox backend fields not owned by sandbox modules;
- full-profile sidecar assumptions;
- startup migration cleanup flags.

## 15. Docker Architecture

Docker must select a module profile. It must not ship every possible runtime asset.

### 15.1 App image

Create a profile-aware app Dockerfile, for example:

```text
docker/Dockerfile.app
```

Build arguments:

```Dockerfile
ARG OXIDE_PROFILE=profile-embedded-opencode-local
ARG CARGO_FEATURES="profile-embedded-opencode-local"
ARG BINARIES="oxide-agent"
```

Build command:

```bash
docker build \
  -f docker/Dockerfile.app \
  --build-arg CARGO_FEATURES="profile-embedded-opencode-local" \
  --build-arg BINARIES="oxide-agent" \
  -t oxide-agent:embedded-opencode-local \
  .
```

The Dockerfile must:

- build with `--no-default-features`;
- pass the exact profile feature list;
- copy only selected binaries;
- copy only selected external binaries/assets;
- use a minimal runtime base;
- not include MCP binaries unless selected;
- not include sandbox daemon unless selected;
- not include browser assets unless selected;
- not install SSH client unless SSH MCP or selected transport requires it;
- not rely on runtime env flags to hide unused capabilities.

### 15.2 Full image

A full image remains valid, but it is just a composition of modules:

```bash
cargo build --release \
  --no-default-features \
  --features "profile-full"
```

Full is not the default architecture.

### 15.3 Embedded image

Embedded image requirements:

- selected app binary only;
- S3/R2 durable storage runtime path;
- selected LLM provider dependencies only;
- no AWS SDK unless `storage-s3-r2` is selected;
- no RMCP binaries if MCP modules are absent;
- no sandbox daemon if sandboxd client/backend is absent;
- no searxng/browser-use sidecars;
- no fat sandbox image unless selected.

### 15.4 MCP binaries

MCP binary download/copy must move to module-specific Docker stages or profile-specific build logic.

Examples:

- `integration-mcp-jira` downloads/copies Jira MCP binary;
- `integration-mcp-mattermost` downloads/copies Mattermost MCP binary;
- `integration-ssh-mcp` downloads/copies SSH MCP binary.

If the feature is absent, the binary must not exist in the final image.

### 15.5 Sandbox images

Create sandbox Dockerfiles or generated Dockerfiles:

```text
docker/sandbox/Dockerfile.minimal
docker/sandbox/Dockerfile.exec
docker/sandbox/Dockerfile.media
docker/sandbox/Dockerfile.dev
```

The selected profile chooses the image.

No profile may silently use the fat image. Heavy packages must be tied to selected modules.

## 16. Docker Compose Architecture

Compose files must be profile-specific or generated from module service declarations.

### 16.1 Required compose profiles

Create at least:

```text
docker/compose.full.yml
docker/compose.embedded-opencode-local.yml
docker/compose.dev.yml
docker/compose.search.yml
docker/compose.media.yml
```

Alternatively, generate them from:

```text
profiles/*.toml
```

and module `ComposeRequirement` declarations.

### 16.2 Compose rules

- No `searxng` service unless `tool-searxng` is enabled.
- No `browser-use` service unless `tool-browser-use` is enabled.
- No `sandboxd` service unless `sandbox-backend-sandboxd-client` is enabled.
- No Docker socket mount unless a Docker-based sandbox backend or sandbox daemon is enabled.
- No MCP sidecar/binary unless the corresponding MCP integration is enabled.
- No browser-use volume unless browser module is enabled.
- No sandbox image build unless a sandbox backend/tool requires it.
- No S3/R2-related env/config unless `storage-s3-r2` is enabled.
- Local persistent volumes must not be used for durable agent state; volumes are allowed only for selected transient workspace, socket, cache, or sidecar modules.

### 16.3 Example embedded compose

```yaml
services:
  oxide-agent:
    image: oxide-agent:embedded-opencode-local
    env_file: .env.embedded
    volumes:
      - ./config:/app/config:ro
```

No searxng, no browser-use, no sandboxd, no Docker socket, no MCP services.

### 16.4 Example full compose

Full compose may include:

- `oxide-agent`;
- `sandboxd`;
- sandbox image build;
- `searxng`;
- browser-use bridge;
- selected MCP binaries/services;
- selected volumes.

Full compose must still be generated from module declarations or maintained as a profile-specific composition. It must not be the base for all profiles.

## 17. Deletion Plan

Because backward compatibility is not required, delete obsolete paths instead of wrapping them.

### 17.1 Delete legacy registry paths

Delete or fully replace:

- `oxide-agent-core/src/agent/registry.rs` legacy provider registry;
- legacy `ToolProvider` dispatch as the primary execution model;
- `ProviderRuntimeExecutor` legacy-to-typed wrapper;
- `build_tool_registry(...)` vs `build_tool_runtime_registry(...)` split;
- duplicate typed/legacy registration code;
- duplicate skip-on-conflict registration behavior.

The new typed `ToolRegistry` must be the only tool registry.

### 17.2 Delete hardcoded provider registration

Delete:

- global provider import/re-export as the registration mechanism;
- global `LlmClient::new` hardcoded provider insertion chain;
- global provider alias lists;
- config credential checks that assume all providers are compiled.

Replace with provider modules.

### 17.3 Delete hardcoded R2 startup

Delete:

- direct `R2Storage::new(settings)` from Telegram runner startup;
- startup maintenance functions that require concrete `R2Storage`;
- global R2 config fields from app settings;
- unconditional AWS SDK dependencies.

Replace with storage modules.

### 17.4 Delete hardcoded sandbox assumptions

Delete:

- broad sandbox tool provider that mixes exec, fileops, delivery, list, and recreate;
- runtime-only `SANDBOX_BACKEND` selection as the architectural mechanism;
- unconditional Bollard dependency;
- unconditional sandboxd build/copy;
- fat sandbox image as default;
- Docker Compose stack log coupling from minimal builds.

Replace with sandbox backend and tool modules.

### 17.5 Delete unconditional sidecars and binaries

Delete:

- unconditional searxng Compose service from non-search profiles;
- unconditional sandboxd service from profiles that do not select it;
- unconditional MCP binary downloads/copies from the app Dockerfile;
- browser-use commented pseudo-profile from the base compose file.

Replace with module-declared service requirements.

### 17.6 Delete compatibility and migration code

Delete:

- old env compatibility aliases;
- deprecated config fields;
- temporary migration switches;
- legacy fallback response shapes in SSH/file tools;
- startup persisted tool drift cleanup if its only purpose is old deployment migration;
- backward-compatibility re-exports;
- unused wrappers/adapters;
- unused default features.

Do not keep compatibility shims.

### 17.7 Delete transport composition responsibilities

Delete from transport startup:

- concrete storage backend construction;
- provider-specific LLM construction;
- sandbox backend selection;
- MCP topology decisions;
- global cleanup behavior unrelated to the transport.

Transport modules must only start transports with a prepared `RuntimeContext`.

## 18. Implementation Plan

### Milestone 1: Dependency and Feature Audit

Deliverables:

- Inventory all crates and dependencies.
- Mark each dependency as core, optional-light, optional-heavy, dev-only, or test-only.
- Create the new atomic feature naming map.
- Move heavy dependencies to `optional = true`.
- Set `default = []` for core modular crates.
- Add initial profile features.
- Add CI jobs for minimal profile builds.
- Add `cargo tree` deny checks for excluded dependency groups.

Required checks:

```bash
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-no-sandbox
cargo check --workspace --no-default-features --features profile-search-only
cargo tree -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local
```

Acceptance for this milestone:

- Minimal profile includes AWS SDK only through the selected `storage-s3-r2` module and does not include unselected storage dependencies.
- No-sandbox profile does not include Bollard.
- No-MCP profile does not include RMCP.
- Provider-specific profile does not include unrelated provider SDKs.

### Milestone 2: New Capability Module System

Deliverables:

- Add `CapabilityModule`, `ModuleRegistry`, `CompiledCapabilityManifest`, and `EnabledCapabilityManifest`.
- Add deterministic `compiled_modules()` feature-gated construction.
- Add config validation against compiled modules.
- Add CLI/API to print compiled and enabled capabilities.
- Introduce one typed `ToolRegistry` as the only target registry.
- Delete legacy registry paths once replacement modules exist.

Acceptance:

- Registry construction is deterministic.
- Duplicate capability IDs fail.
- Duplicate tool names fail.
- Unknown config module IDs fail.
- Non-compiled module config fails.

### Milestone 3: LLM Provider Modularization

Deliverables:

- Create provider module trait and provider registry.
- Move each LLM provider behind its own feature.
- Move provider aliases into provider modules.
- Move model capability metadata into provider modules.
- Remove embedding providers and their global match chain.
- Replace global provider insertion chain with module factories.
- Add provider-specific config schemas.

Acceptance:

- `llm-opencode-go` build compiles only OpenCode Go provider code and shared protocol code.
- A config referencing the removed direct Google Gemini provider fails because it is not a compiled provider.
- Provider aliases are registered only when the provider module is compiled and enabled.
- Global hardcoded provider match chains are removed.

### Milestone 4: Tool Modularization

Deliverables:

- Split broad providers into atomic tool modules.
- Move tool definitions and executors together.
- Replace legacy `ToolProvider` with typed `ToolExecutor` modules.
- Remove `ProviderRuntimeExecutor`.
- Remove typed/legacy dual registration.
- Add capability dependency declarations to every tool.
- Add snapshot tests for tool lists per profile.

Required first split:

- sandbox fileops vs sandbox exec vs file delivery vs recreate;
- webfetch vs Tavily vs SearXNG vs browser-use;
- media audio vs image vs video;
- MCP Jira vs Mattermost vs SSH;
- manager/control-plane submodules.

Acceptance:

- There is exactly one tool registry.
- There is exactly one registration path for each tool.
- Embedded profile excludes `execute_command` unless `tool-sandbox-exec` is explicitly enabled.
- Search-only profile excludes browser-use and MCP tools.

### Milestone 5: Storage Modularization

Deliverables:

- Create `StorageBackendModule` trait.
- Keep local filesystem storage out of the durable backend set; local filesystem may be used only for transient runtime workspace data.
- Move R2/AWS code into the `storage-s3-r2` module.
- Remove concrete `R2Storage` from transport startup.
- Split or wrap storage traits into smaller capability-specific interfaces.
- Update tools and runtime to consume storage interfaces.

Acceptance:

- `storage-s3-r2` is the only durable storage backend.
- Builds without `storage-s3-r2` exclude AWS SDK but do not provide a production durable storage startup path.
- Telegram transport runs with storage supplied by the module registry, not by direct R2 construction.
- S3/R2 config appears only under `storage/r2` or its module-owned environment variables.
- Storage backend selection is validated by module registry.

### Milestone 6: Sandbox Modularization

Deliverables:

- Create sandbox backend traits.
- Split Docker direct backend and sandboxd client backend.
- Make Bollard optional.
- Make broker protocol optional.
- Move sandboxd binary behind `sandbox-daemon` feature.
- Split sandbox tools into fileops, exec, recreate, file delivery, diagnostics.
- Create minimal/exec/media/dev sandbox image variants.
- Move SSH cleanup into SSH module.

Acceptance:

- No-sandbox build excludes Bollard and sandbox broker protocol.
- Embedded fileops build excludes `execute_command`.
- Exec profile includes exec intentionally and exposes it in manifest.
- Minimal sandbox image excludes ffmpeg, Python, yt-dlp, nmap, and mtr unless selected.

### Milestone 7: Transport Decoupling

Deliverables:

- Move app composition into a profile-aware bootstrap crate/module.
- Convert Telegram startup to consume `RuntimeContext`.
- Convert web transport to consume the same `RuntimeContext` model.
- Add CLI or HTTP transport module if selected by roadmap.
- Move transport-specific file delivery into transport module capabilities.

Acceptance:

- Transports do not construct concrete storage backends.
- Transports do not construct LLM providers directly.
- Transports do not select sandbox backend directly.
- Multiple transports can be included or excluded by feature profile.

### Milestone 8: Docker and Compose Profiles

Deliverables:

- Create profile-aware app Dockerfile.
- Remove hardcoded Jira/Mattermost feature build from base image.
- Move MCP binary downloads to module/profile-specific stages.
- Create profile-specific Compose files or generator.
- Create sandbox image variants.
- Add Docker build CI for core profiles.
- Add `docker compose config` validation for profile compose files.

Acceptance:

- Embedded compose starts only selected services.
- No searxng in embedded compose unless selected.
- No sandboxd unless selected.
- No MCP binaries in images unless selected.
- Full profile remains buildable as module composition.

### Milestone 9: Tests, CI, and Size Budgets

Deliverables:

- Capability manifest snapshot tests.
- Enabled module snapshot tests by config/profile.
- Config validation tests.
- Tool list snapshot tests.
- Cargo dependency deny checks.
- Binary size checks.
- Docker image size checks.
- Compose topology checks.
- Smoke tests for key profiles.

Required profile matrix:

```bash
cargo check --no-default-features --features profile-embedded-opencode-local
cargo check --no-default-features --features profile-lite
cargo check --no-default-features --features profile-search-only
cargo check --no-default-features --features profile-no-sandbox
cargo check --no-default-features --features profile-media-enabled
cargo check --no-default-features --features profile-full
```

Acceptance:

- CI blocks dependency leakage.
- CI blocks accidental tool registry drift.
- CI blocks binary/image size regressions beyond configured budget.
- CI blocks compose service drift.

## 19. Testing Strategy

### 19.1 Build tests

Run profile builds in CI:

```bash
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-lite
cargo check --workspace --no-default-features --features profile-search-only
cargo check --workspace --no-default-features --features profile-no-sandbox
cargo check --workspace --no-default-features --features profile-media-enabled
cargo check --workspace --no-default-features --features profile-full
```

Add release builds for size-tracked profiles:

```bash
cargo build --release -p oxide-agent --no-default-features --features profile-embedded-opencode-local
cargo build --release -p oxide-agent --no-default-features --features profile-full
```

### 19.2 Dependency leakage tests

Use `cargo tree` checks to ensure excluded dependencies are absent.

Examples:

```bash
cargo tree -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local | grep -q aws-sdk-s3 && exit 1
cargo tree -p oxide-agent-core --no-default-features --features profile-no-sandbox | grep -q bollard && exit 1
cargo tree -p oxide-agent-core --no-default-features --features profile-search-only | grep -q rmcp && exit 1
```

Replace grep scripts with a checked CI utility if needed.

### 19.3 Registry snapshot tests

For each profile, snapshot:

- compiled module IDs;
- compiled capability IDs;
- enabled module IDs for default config;
- registered tool names;
- registered LLM provider IDs;
- registered storage backend IDs;
- registered sandbox backend IDs;
- external service requirements.

Tests must fail on accidental capability drift.

### 19.4 Config validation tests

Required cases:

- unknown module ID fails;
- non-compiled module config fails;
- compiled but disabled module does not register;
- missing required module dependency fails;
- conflicting modules fail;
- provider route references non-compiled provider fails;
- provider route references disabled provider fails;
- tool requiring sandbox exec fails without exec backend;
- storage backend missing fails;
- multiple primary storage backends fail unless explicitly allowed.

### 19.5 Tool availability tests

Required cases:

- disabled tools are absent from final tool list;
- non-compiled tools cannot be enabled by config;
- `execute_command` absent in embedded/lite default profile;
- MCP tools absent when MCP features are absent;
- browser tools absent when browser feature is absent;
- media tools absent unless media feature is selected;
- Tavily tools absent unless `tool-tavily` is compiled and enabled.

### 19.6 Docker tests

Required cases:

```bash
docker build -f docker/Dockerfile.app \
  --build-arg CARGO_FEATURES="profile-embedded-opencode-local" \
  -t oxide-agent:test-embedded .

docker build -f docker/Dockerfile.app \
  --build-arg CARGO_FEATURES="profile-full" \
  -t oxide-agent:test-full .
```

Image inspection tests must verify:

- no MCP binaries in embedded image;
- no sandboxd binary unless selected;
- no unexpected runtime package groups;
- selected binary exists;
- capability manifest command runs.

### 19.7 Compose tests

Required cases:

```bash
docker compose -f docker/compose.embedded-opencode-local.yml config
docker compose -f docker/compose.full.yml config
docker compose -f docker/compose.search.yml config
```

Tests must assert service absence/presence:

- embedded has no searxng;
- embedded has no browser-use;
- embedded has no sandboxd unless selected;
- search has searxng only if `tool-searxng` profile uses internal searxng;
- full has all selected services.

### 19.8 Size budgets

Add configured budgets per profile.

Initial budgets should be established after the first successful modular build. They must then be enforced.

Track:

- release binary size;
- compressed image size;
- uncompressed image size;
- number of compiled capabilities;
- dependency count;
- high-risk dependency count.

## 20. Acceptance Criteria

The refactor is complete when all criteria below are true.

### 20.1 Architecture criteria

- There is exactly one module registration path.
- There is exactly one typed tool registry.
- No legacy tool registry remains.
- No legacy-to-typed tool wrapper remains.
- No duplicated typed/legacy tool registration remains.
- Adding a new tool requires adding one module and one feature, plus optional profile inclusion.
- Adding a new provider requires adding one provider module and one feature, plus optional profile inclusion.
- Global provider match chains are removed.
- Provider aliases are owned by provider modules.
- Runtime config cannot enable absent compile-time modules.

### 20.2 Minimal build criteria

The embedded profile compiles and runs with:

```bash
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
```

The embedded profile must not include:

- AWS SDK except through the selected `storage-s3-r2` module;
- unselected durable storage backends;
- browser-use;
- searxng sidecar requirement unless selected;
- Jira MCP;
- Mattermost MCP;
- SSH MCP unless selected;
- unused LLM SDKs;
- Bollard unless a Docker sandbox backend is selected;
- sandbox command execution unless explicitly selected;
- media-heavy tooling unless selected.

### 20.3 Provider criteria

- Only selected LLM providers are compiled.
- Only selected provider aliases are present.
- Provider-specific config appears only under that provider module.
- Config referencing a non-compiled provider fails at startup.
- Config referencing a disabled provider fails route validation.

### 20.4 Tool criteria

- Only selected tools are present in the registry.
- Disabled tools are not visible in the tool list.
- Non-compiled tools cannot be enabled by config.
- `execute_command` is absent from embedded/lite defaults.
- Sandbox file operations can be enabled without sandbox command execution.
- Search tools can be enabled without browser tools.
- MCP tools can be enabled independently per integration.
- Media audio/image/video can be enabled independently.

### 20.5 Storage criteria

- Persistent local storage is not an acceptance target.
- `storage-s3-r2` is the only production durable storage module.
- AWS SDK appears only when `storage-s3-r2` is selected.
- Builds without `storage-s3-r2` may compile core/runtime helpers and tests without AWS SDK, but they must not expose a production durable storage startup path.
- Transport startup does not construct concrete `R2Storage`.
- Tools consume storage traits/interfaces, not concrete backend types.

### 20.6 Sandbox criteria

- No-sandbox profile compiles without Bollard and sandbox broker protocol.
- Sandboxd binary is built only with `sandbox-daemon`.
- Docker socket is required only by Docker sandbox backend or sandbox daemon service.
- Minimal sandbox image excludes ffmpeg, Python, yt-dlp, nmap, mtr, and browser packages unless selected.
- Sandbox exec and sandbox file operations are separate modules.

### 20.7 Docker and Compose criteria

- Embedded Docker image contains only selected binaries and runtime assets.
- Full Docker image remains possible as a module composition.
- MCP binaries are absent unless corresponding MCP modules are enabled.
- Compose embedded profile starts only required services.
- Compose does not start searxng unless selected.
- Compose does not start browser-use unless selected.
- Compose does not start sandboxd unless selected.
- Persistent volumes are declared only for selected modules.

### 20.8 CI criteria

- CI verifies all required profile builds.
- CI verifies dependency absence for minimal/no-sandbox/no-MCP/provider-specific builds.
- CI verifies registry snapshots.
- CI verifies config validation behavior.
- CI verifies Docker builds.
- CI verifies Compose topology.
- CI enforces binary and image size budgets.

## 21. Risks and Tradeoffs

### 21.1 Breaking all existing deployments

Risk:

- Config names, env names, Docker topology, and binary behavior will break existing deployments.

Decision:

- This is acceptable. Backward compatibility and migrations are not goals.

Mitigation:

- Provide new example configs and profile-specific Compose files.
- Provide `oxide-agent config example --profile ...`.

### 21.2 Feature explosion

Risk:

- Atomic modules can create many Cargo features.

Mitigation:

- Use strict naming convention.
- Keep atomic features for code/dependency boundaries.
- Provide profile features for common combinations.
- Generate capability manifests.
- Add module templates.

### 21.3 Config complexity

Risk:

- Module-scoped config can become verbose.

Mitigation:

- Provide profile defaults.
- Generate example config.
- Validate config with clear errors.
- Keep module IDs stable and predictable.

### 21.4 Too many tiny modules

Risk:

- Over-splitting can make development harder.

Mitigation:

- Split by dependency/risk/runtime boundary, not every function blindly.
- Allow profile composition features.
- Allow a crate to contain multiple lightweight modules if dependencies are shared and boundaries stay clear.

### 21.5 Optional dependency mistakes

Risk:

- A heavy dependency can leak through a shared module.

Mitigation:

- CI `cargo tree` deny checks.
- Review feature graph in Milestone 1.
- Avoid importing provider modules from shared `mod.rs` without cfg gates.

### 21.6 Docker profile drift

Risk:

- Cargo profiles, Dockerfiles, and Compose files can diverge.

Mitigation:

- Generate Docker/Compose metadata from module requirements where practical.
- Snapshot service requirements per profile.
- Keep profile definitions in one place.

### 21.7 Hidden runtime dependency leakage

Risk:

- A tool may compile cleanly but require a sidecar/package not declared.

Mitigation:

- Require every module to declare Docker and Compose requirements.
- Validate profile manifests against module requirements.
- Add smoke tests for key tools per profile.

### 21.8 Media tools are inherently heavy

Risk:

- Audio/image/video tools may drag in large runtime requirements.

Mitigation:

- Split audio, image, and video capabilities.
- Keep media tools out of default embedded/lite profiles.
- Use explicit `profile-media-enabled`.

### 21.9 Manager/control-plane complexity

Risk:

- Manager tools currently span many domains and may resist clean separation.

Mitigation:

- Split internally by subdomain.
- Keep `manager-control-plane` as a profile composition feature only.
- Require submodules to declare dependencies like storage, sandbox admin, and topic policy.

## 22. Open Questions

The implementation team must decide the following.

1. Should profile definitions live only in Cargo features, only in external `profiles/*.toml`, or both?

   Recommendation: use both. Cargo features define compile-time capability sets. `profiles/*.toml` define runtime defaults and Docker/Compose generation inputs.

2. Should Compose files be generated from module service declarations?

   Recommendation: yes for long-term consistency. Initially, maintain profile-specific Compose files with snapshot tests.

3. Should embedded default use docker-direct sandbox, sandboxd, or no sandbox backend?

   Recommendation: embedded default should include no command execution. If file operations are required, prefer sandboxd client only when an external sandboxd is explicitly selected.

4. Should media tools be split by audio/image/video?

   Recommendation: yes. Their provider and runtime requirements differ.

5. Should MCP integrations be separate crates?

   Recommendation: yes if their dependencies or generated clients are heavy. At minimum, they must be feature-gated modules.

6. Should transports live in separate binaries or one binary with feature-gated transports?

   Recommendation: support both. The composition root can build one selected transport binary for minimal deployments and a multi-transport binary for full/dev deployments.

7. Should local storage be filesystem or SQLite?

  Decision: neither. Persistent local storage is not supported.

  Oxide Agent must use S3/R2-compatible object storage as the single canonical durable storage backend. The agent is designed for stateless Linux nodes where the local disk can be destroyed at any time. A node must be recoverable by redeploying the binary/container and providing the required `.env` configuration for the S3/R2 backend.

  Local filesystem usage is allowed only for transient runtime data: temporary files, sandbox workdirs, download buffers, upload staging, and disposable cache. No durable agent state may depend on local filesystem persistence.

  SQLite must not be used as a required storage backend. Filesystem storage must not be used as a durable backend. All durable state, including conversations, todos, reminders, wiki/memory data, artifacts, sandbox inputs/outputs, generated files, and manifests, must be stored in S3/R2.

  Required storage decision:

  - `storage-s3-r2` is the only durable storage module.
  - `storage-local-fs` is not a durable storage module; it is only transient runtime workspace.
  - `storage-sqlite` is not part of the target architecture.
  - Stateless recovery from `.env` + S3/R2 bucket is a hard acceptance criterion.

The implementation must treat S3/R2 as the source of truth. Runtime startup must fail loudly if S3/R2 configuration is missing or invalid.

8. Should stack log access be a sandbox diagnostic module or a manager diagnostic module?

   Recommendation: model it as `tool-stack-logs` requiring `SandboxDiagnostics` and Docker/Compose metadata.

9. Should `chatgpt-login` remain a separate binary?

   Recommendation: keep it only behind a provider/auth feature that requires it. Do not copy it into images that do not use that auth flow.

10. Should old persisted tool names be transformed?

   Recommendation: no migration. Old persisted state can be discarded or ignored by clean deployments.

11. Should embeddings and skills remain in the codebase?

   Decision: no. Remove embeddings and skills from the target architecture.

   RECON shows that embeddings are used only by the legacy skills subsystem for semantic matching of user messages against markdown skill descriptions. They are not used for wiki memory, durable memory retrieval, artifact search, vector storage, or any required runtime path.

   The current skills subsystem is effectively inactive: `AgentExecutor` initializes `skill_registry` as `None`, and the prompt composer accepts `SkillRegistry` but does not use it to build the system prompt. The repository also does not contain an active root `skills/` markdown directory, while Dockerfile/docs still reference it as a legacy artifact.

   Required action:

   - Delete `agent/skills/*`.
   - Delete `llm/embeddings.rs`.
   - Remove embedding fields and methods from `LlmClient`.
   - Remove embedding config fields and environment variables.
   - Remove `EMBEDDING_*`, `SKILL_*`, `SKILLS_DIR`, and `.embeddings_cache` behavior.
   - Remove Dockerfile `COPY skills/ /app/skills/`.
   - Remove docs that describe Mistral or any embedding model as required for skill selection.
   - Remove embedding provider capabilities from the modular architecture.
   - Do not add `embedding-provider/*` modules.
   - Do not add local vector DB or embedding cache support.
   - Do not preserve old skill names or old skill state.

   All durable context must come from explicit modules such as S3/R2-backed wiki memory, AGENTS.md/topic instructions, profile prompt instructions, and enabled tools. If prompt packs are needed later, they must be deterministic profile-selected prompt modules, not embedding-selected skills.

   Acceptance criteria:

   - No production code path references `EmbeddingProvider`, `EmbeddingTaskType`, `generate_embedding`, `probe_embedding_dimension`, `SkillRegistry`, `SkillMatcher`, or `EmbeddingService`.
   - Minimal/stateless builds contain no embedding provider code.
   - Runtime startup does not accept or require `EMBEDDING_*` or `SKILL_*` environment variables.
   - Docker images do not copy a `skills/` directory.
   - The agent remains fully recoverable from `.env` plus S3/R2 bucket, with no local embedding cache or local skill state.

## 23. Proposed Final Repository Shape

Recommended target layout:

```text
crates/
  oxide-agent-core/
    src/
      app/
        bootstrap.rs
        context.rs
        runtime.rs
      capabilities/
        ids.rs
        manifest.rs
        module.rs
        registry.rs
        requirements.rs
      config/
        loader.rs
        schema.rs
        validation.rs
        env.rs
      llm/
        router.rs
        traits.rs
        model_capabilities.rs
      tools/
        registry.rs
        executor.rs
        spec.rs
      storage/
        traits.rs
      sandbox/
        traits.rs
      events/
      policies/

  oxide-agent-modules/
    src/
      lib.rs
      llm/
        chatgpt.rs
        groq.rs
        mistral.rs
        minimax.rs
        zai.rs
        nvidia.rs
        opencode_go.rs
        openrouter.rs
      storage/
        local.rs
        r2.rs
      sandbox/
        docker_direct.rs
        sandboxd_client.rs
        daemon.rs
      tools/
        todos.rs
        compression.rs
        delegation.rs
        agents_md.rs
        reminder.rs
        wiki_memory.rs
        webfetch_md.rs
        tavily.rs
        searxng.rs
        browser_use.rs
        sandbox_fileops.rs
        sandbox_exec.rs
        sandbox_recreate.rs
        file_delivery.rs
        stack_logs.rs
        media_audio.rs
        media_image.rs
        media_video.rs
        ytdlp.rs
        tts_kokoro.rs
        tts_silero.rs
      integrations/
        mcp_jira.rs
        mcp_mattermost.rs
        ssh_mcp.rs
      manager/
        forum_topics.rs
        topic_binding.rs
        topic_context.rs
        topic_agents_md.rs
        topic_infra.rs
        topic_sandbox.rs
        agent_profiles.rs
        tool_policy.rs
        hook_policy.rs
        private_secret_probe.rs

  oxide-agent-runtime/
    src/
      session.rs
      progress.rs
      transport_runtime.rs

  oxide-agent-transport-telegram/
    src/
      module.rs
      runner.rs
      handlers/

  oxide-agent-transport-web/
    src/
      module.rs
      server.rs

  oxide-agent-bin/
    src/
      main.rs
      commands/
        capabilities.rs
        config_schema.rs

  oxide-agent-sandboxd/
    src/
      main.rs

docker/
  Dockerfile.app
  compose.full.yml
  compose.embedded-opencode-local.yml
  compose.dev.yml
  compose.search.yml
  compose.media.yml
  sandbox/
    Dockerfile.minimal
    Dockerfile.exec
    Dockerfile.media
    Dockerfile.dev

profiles/
  full.toml
  embedded-opencode-local.toml
  lite.toml
  search-only.toml
  no-sandbox.toml
  media-enabled.toml

scripts/
  check-cargo-tree-deny.sh
  generate-compose.rs
  generate-config-example.rs

xtask/
  src/
    main.rs
    capabilities.rs
    docker.rs
    size.rs
```

Alternative acceptable layout:

- split heavy modules into separate crates, such as `oxide-agent-storage-r2` or `oxide-agent-integration-mcp-jira`, if compile times or dependency isolation require it.

Hard requirement:

- shared core must not import heavy modules unconditionally.

## 24. Example Profiles

### 24.1 `full`

Purpose:

- development and maximum capability deployments.

Includes:

- Telegram transport;
- web transport if needed;
- S3/R2 durable storage;
- all LLM providers;
- all search/fetch/browser tools;
- sandbox fileops and exec;
- sandboxd and Docker direct backends;
- media tools;
- yt-dlp;
- TTS;
- Jira MCP;
- Mattermost MCP;
- SSH MCP;
- manager/control-plane tools;
- full Compose sidecars.

Command:

```bash
cargo build --release --no-default-features --features profile-full
```

### 24.2 `embedded-opencode-local`

Purpose:

- small deployment using OpenCode Go provider and S3/R2 durable storage.

Includes:

- Telegram transport or selected lightweight transport;
- S3/R2 durable storage;
- OpenCode Go LLM provider;
- todos;
- agents.md;
- reminders;
- wiki memory;
- webfetch-md;
- Tavily if configured;
- sandbox file operations only if explicitly selected.

Excludes by default:

- all unused LLM SDKs;
- unselected storage backends;
- MCP integrations;
- browser-use;
- searxng sidecar;
- sandbox command execution;
- sandbox fat image;
- media-heavy tools;
- yt-dlp;
- TTS sidecars;
- sandboxd unless explicitly selected.

Command:

```bash
cargo build --release --no-default-features --features profile-embedded-opencode-local
```

### 24.3 `search-only`

Purpose:

- agent with web search/fetch tools and one LLM provider.

Includes:

- selected transport;
- S3/R2 durable storage;
- one selected LLM provider;
- `webfetch-md`;
- Tavily or SearXNG depending on profile variant.

Excludes:

- browser-use unless selected;
- MCP;
- sandbox;
- media;
- manager/control-plane;
- non-selected storage backends.

### 24.4 `no-sandbox`

Purpose:

- deployments where sandboxing is disallowed, unnecessary, or externally managed.

Includes:

- selected transport;
- S3/R2 durable storage;
- selected LLM provider;
- non-sandbox tools such as todos, reminders, webfetch, wiki memory.

Excludes:

- Bollard;
- sandboxd;
- Docker socket mounts;
- sandbox image builds;
- all sandbox tools;
- stack logs.

### 24.5 `media-enabled`

Purpose:

- deployments that intentionally enable media analysis/transcription.

Includes:

- selected transport;
- S3/R2 durable storage;
- media-capable LLM provider;
- audio transcription module if selected;
- image description module if selected;
- video description module if selected;
- media sandbox image packages only if required.

Excludes unless explicitly selected:

- browser-use;
- MCP;
- full sandbox exec;
- unrelated search sidecars.

### 24.6 `provider-specific-opencode-go`

Purpose:

- verify provider isolation and dependency elimination.

Includes:

- OpenCode Go provider;
- minimal routing/config support;
- optional test/noop storage helpers only for non-production checks;
- no unrelated provider SDKs.

Acceptance:

- Former direct Gemini SDK code stays absent; `zai-rs` and `async-openai` are absent unless explicitly required by selected modules.

## 25. Required Output Format

The implementation agent or engineering team consuming this PRD must produce code and repository changes that preserve these output guarantees:

- profile builds are reproducible from Cargo features;
- compiled capabilities can be printed as deterministic JSON;
- enabled capabilities can be printed as deterministic JSON for a config;
- config schema can be generated for compiled modules;
- Docker and Compose assets correspond to selected modules;
- CI proves minimal dependency absence;
- no legacy registry or compatibility path remains.

The final implementation must not introduce migration layers, deprecated wrappers, compatibility aliases, or duplicate old/new registration systems. The correct resolution for old architecture is deletion and replacement with capability modules.
