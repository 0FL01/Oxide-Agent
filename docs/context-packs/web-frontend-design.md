# Context Pack: Oxide Agent Web Console Frontend

> Atomized reference for LLM design agents. All information grounded in source code.

---

## 1. Architecture Overview

- **Framework**: Leptos CSR (Client-Side Rendering) compiled to WebAssembly via Trunk.
- **Language**: 100% Rust. No TypeScript, React, Vue, or Svelte.
- **Build**: `Trunk.toml` builds to `crates/oxide-agent-web-ui/dist/`. The backend serves these static files via SPA fallback.
- **Crate**: `crates/oxide-agent-web-ui/` (frontend) + `crates/oxide-agent-web-contracts/` (shared API types).
- **Server**: `crates/oxide-agent-transport-web/` (Axum-based, serves SPA + REST API + SSE).

### Source files (all in `crates/oxide-agent-web-ui/src/`)

| File | Purpose |
|------|---------|
| `main.rs` | WASM entry point |
| `app.rs` | Root component, auth-gated routing |
| `routes.rs` | Route enum: Login, Register, Bootstrap, App, Session(id), Settings, NotFound |
| `components.rs` | AppLayout (2-column grid), Header, StatusBadge, EmptyState, ErrorBanner |
| `sessions.rs` | SessionSidebar (left panel): list, search, create, delete sessions |
| `tasks.rs` | TaskConsole, SessionWorkspace, TaskCard, AgentActivity, ToolCard variants, TodosCard, AgentEventCard, TaskInputEditForm, Composer |
| `sse.rs` | SSE client: EventSource-based streaming with reconnect, backfill, keepalive |
| `auth.rs` | AuthState, AuthContext, LoginPage, RegisterPage, BootstrapPage, SettingsPage |
| `api.rs` | ApiClient: typed HTTP wrapper over gloo-net with CSRF + cookie auth |
| `markdown.rs` | Markdown rendering (comrak) + XSS sanitization (ammonia) + code copy buttons |
| `utils.rs` | spawn_ui, browser_pathname, navigate, friendly_time |
| `styles.css` | Complete design system (~1824 lines) |

---

## 2. Design System (CSS)

### 2.1 Theme: "Neutral Dark" (ChatGPT-inspired)

- `color-scheme: dark`
- No warm undertones. Pure neutral grays.

### 2.2 Color Tokens

```
Backgrounds:
  --bg-root:       #171717     (page background)
  --bg-panel:      #171717     (sidebar, panels)
  --bg-panel-soft: #212121     (elevated surfaces, inputs, tool cards)
  --bg-panel-hover:#2f2f2f     (hover states)

Borders:
  --border-subtle: #2a2a2a     (default dividers)
  --border-strong: #3a3a3a     (input borders, focused elements)

Text:
  --text-main:  #ececec         (primary text)
  --text-muted: #8e8e93         (secondary text)
  --text-faint: #6e6e73         (tertiary, timestamps, labels)

Accents:
  --accent-blue:   #10a37f      (primary action, links, running state)
  --accent-green:  #10a37f      (success = same as blue/teal)
  --accent-yellow: #d8a21e      (warnings, waiting)
  --accent-red:    #ef4444      (errors, failed, delete)
```

### 2.3 Typography

- **Sans**: `'Inter', -apple-system, BlinkMacSystemFont, system-ui, sans-serif`
- **Mono**: `'JetBrains Mono', 'SF Mono', ui-monospace, monospace`
- **Base size**: `13px`, line-height `1.5`
- **Antialiased**: `-webkit-font-smoothing: antialiased`

### 2.4 Spacing Scale

```
--space-1: 4px    --space-5: 20px
--space-2: 8px    --space-6: 24px
--space-3: 12px   --space-8: 32px
--space-4: 16px   --space-10: 40px
                   --space-12: 48px
```

### 2.5 Layout Constants

```
--sidebar-width:   260px
--metrics-width:   300px
--topbar-height:   48px
--input-min-height: 150px
```

### 2.6 Design Principles

- **Zero border-radius**: All elements use `border-radius: 0`. Sharp, brutalist edges.
- **No shadows/elevation**: Flat design. Depth via background color shifts only.
- **Minimal borders**: Borders are near-invisible (`#2a2a2a`). Separated by subtle 1px lines.
- **Restrained color**: Monochrome grays dominate. Color used only for status/signal.
- **Monospace for data**: Timestamps, IDs, tool names, metrics, labels all use `--font-mono`.
- **Button style**: Flat, bordered, no radius. Primary = solid accent. Secondary = transparent.
- **Scrollbar**: 6px thin, white-on-transparent. Custom webkit + Firefox.
- **Transitions**: Fast `0.12s` for background/border/color. `0.15s` for input focus.

---

## 3. Layout Structure

### 3.1 App Shell (2-column grid)

```
+-------------------+------------------------------------------+
|                   |  TopBar (48px)                            |
|  Sidebar (260px)  |  "Oxide Agent"     Settings  [user]      |
|                   +------------------------------------------+
|  Sidebar Header   |  SessionWorkspace                        |
|  "Oxide Agent     |  +--------------------------------------+|
|   v0.1"    [+]    |  | Results Panel (scrollable)          ||
|                   |  |  TaskCard 1                         ||
|  Search input     |  |  TaskCard 2                         ||
|                   |  |  ...                                ||
|  Sessions list    |  +--------------------------------------+|
|  - item 1 (active)|  | Composer                            ||
|  - item 2         |  |  "Agent Prompt" label               ||
|  - item 3         |  |  [textarea]                         ||
|                   |  |  [stats] [Run Agent] [Stop]         ||
+-------------------+------------------------------------------+
```

### 3.2 Responsive breakpoints

- `> 1200px`: Full layout (260px sidebar)
- `1024-1200px`: Sidebar shrinks to 220px
- `< 1024px`: Sidebar hidden, events panel hidden. Single column.

---

## 4. Pages and Routes

| Route | Component | Auth required | Description |
|-------|-----------|---------------|-------------|
| `/login` | `LoginPage` | No | Login form |
| `/register` | `RegisterPage` | No | Registration (if enabled) |
| `/bootstrap` | `BootstrapPage` | No | First admin creation |
| `/app` | `AppLayout` > `EmptyState` | Yes | Main app, no session selected |
| `/app/session/:id` | `AppLayout` > `SessionWorkspace` | Yes | Active session with tasks |
| `/settings` | `SettingsPage` | Yes | Account info + change password |
| `*` | `NotFound` | No | 404 page |

### 4.1 Auth flow

- Cookie-based: `oxide_web_session` (HttpOnly, SameSite=Lax)
- CSRF: `X-CSRF-Token` header on all mutating requests
- On app load: `GET /api/v1/me` to check session validity
- Unauthenticated users redirected to `/login`
- Auth state held in Leptos signal (`AuthState { user, csrf_token, loading, session_expired }`)

---

## 5. Components Detail

### 5.1 SessionSidebar (`sessions.rs`)

- **Header**: "Oxide Agent v0.1" + `[+]` button to create new session
- **Search**: Text input filtering sessions by title or preview text (case-insensitive)
- **Session list**: `<ul>` with `<For>` reactive loop
  - Each item shows: title, status dot (color-coded), preview text, timestamp
  - Active session: left border accent + subtle green background
  - Delete button: appears on hover, uses `window.confirm()` dialog
- **Status dot colors**: idle=faint, running=blue(pulsing), success=green, error=red, warning=yellow
- **Empty state**: "No sessions" + "Create a new session to get started."

### 5.2 SessionWorkspace (`tasks.rs`)

- **Results Panel**: Scrollable task list. Each task renders as a `TaskCard`.
- **Composer**: Bottom-fixed input area with:
  - "Agent Prompt" label
  - Monospace textarea (min 150px, max 360px, resizable)
  - Stats bar: character count, line count, context budget metrics
  - "Run Agent" / "Resume" button + "Stop" button
  - Ctrl+Enter keyboard shortcut to submit
- **Busy state**: "This session is busy. Stop the active task before starting another one."
- **Waiting state**: "The task is waiting for your reply. Sending will resume the same task."

### 5.3 TaskCard (`tasks.rs`)

Timeline-style card with left border:
```
[timestamp]
[user message in muted text]
[Edit input button - only for latest terminal task]
[AgentActivity - inline tool calls, events, todos]
[final_response_markdown - rendered as Markdown]
[error_message - red text]
[pending_user_input - yellow text]
```

### 5.4 AgentActivity (`tasks.rs`)

Inline between user message and final answer. Contains:
- **TodosCard**: Checklist of todo items with status labels (todo/doing/done/blocked/cancelled)
- **ToolCard**: Grouped tool_call + tool_result pairs
- **AgentEventCard**: Non-tool events (reasoning, errors, retries, compaction, etc.)

### 5.5 ToolCard Variants

Three specialized renderers based on tool name:

**ShellToolCard** (`execute_command`):
- Header: [icon] Shell [status] [duration] [exit code] [error]
- Preview: `$ command_preview`
- Expandable body: full command, stdout, stderr, raw JSON

**SearchToolCard** (`web_search`, `tavily_search`):
- Header: [icon] Web search [duration] [result count]
- Preview: first result snippet
- Expandable body: query, numbered result list (title + URL + snippet, max 8)

**GenericToolCard** (fallback):
- Header: [icon] [tool_name] [duration] [exit code]
- Preview: command_preview or first line of stdout
- Expandable body: stdout, stderr, raw JSON

**Tool status icons**: hourglass (running), checkmark (success), cross (failure)

### 5.6 AgentEventCard

Collapsible `<details>` with:
- Kind label (uppercase mono): "reasoning", "error", "compacting", "rate limit retry", etc.
- Title text
- Optional flags: "truncated" (yellow), "redacted" (red)
- Expandable body: pre-formatted text

### 5.7 Composer Stats Bar

When progress data is available, shows a rich context budget display:
```
[Running] [healthy] 12k free · flow 8k · prompt 4k · tools 2k · 3 lines · 42 chars
```
- Budget pill color: ok=green, warn=yellow, over=red
- Running indicator: pulsing blue dot
- Terminal status: green checkmark or red cross

### 5.8 Auth Pages

All use centered `auth-panel` (max 420px width, bordered):
- **Login**: Login + Password + "Sign in" + links to Register/Bootstrap
- **Register**: Login + Password + Confirm + "Create account"
- **Bootstrap**: Login + Password + Bootstrap token + "Create admin"
- **Settings**: Two-column grid: Account info (left) + Change password form (right) + Logout button

---

## 6. SSE Streaming (`sse.rs`)

### 6.1 Connection lifecycle

1. `spawn_task_stream()` spawns two parallel tasks:
   - **Poller**: `poll_task_detail_until_paused_or_terminal()` — polls task detail every 500ms, up to 120 iterations
   - **Streamer**: `run_task_stream()` — manages EventSource connection

2. Streamer loop:
   - Backfill missed events via `GET /events?after_seq=N`
   - Open `EventSource` to `/tasks/:id/stream?after_seq=N`
   - Subscribe to 5 named channels: `snapshot`, `task_event`, `progress`, `task_status`, `keepalive`
   - Process messages via `futures_util::select!` across all 5 channels
   - On disconnect: retry up to 3 attempts with reconnect
   - On terminal event or error limit: set state to `TerminalClosed` or `Disconnected`

### 6.2 SSE event types handled

| Event | Action |
|-------|--------|
| `snapshot` | Extract `last_seq` for replay cursor |
| `task_event` | Deserialize `PersistedTaskEvent`, append to events list (deduplicated by seq) |
| `progress` | Update progress snapshot (token budget, todos, compaction status) |
| `task_status` | On terminal or `WaitingForUserInput`: stop stream, refresh task detail |
| `keepalive` | Periodic refresh of task detail as safety net |

### 6.3 Connection states

`SseConnectionState`: `Disconnected` | `Reconnecting` | `Connected` | `TerminalClosed`

---

## 7. Markdown Rendering (`markdown.rs`)

- **Parser**: comrak (CommonMark) with extensions: tables, strikethrough, tasklists, autolink
- **Sanitizer**: ammonia (HTML sanitizer)
  - Allowed: http, https, mailto URL schemes
  - Images stripped (`<img>` removed)
  - Script/event attributes stripped
  - `<input>` tag allowed (for tasklists checkboxes)
- **Code blocks**: Auto-wrapped with "Copy" button (clipboard API)
- **Security**: Raw HTML from markdown is blocked (`options.render.unsafe = false`)

---

## 8. API Surface (used by frontend)

### 8.1 Auth

| Method | Endpoint | Body | Response |
|--------|----------|------|----------|
| GET | `/api/v1/public-config` | - | `{ registration_enabled, bootstrap_required, build_version }` |
| GET | `/api/v1/me` | - | `{ user: { user_id, login, role }, csrf_token }` |
| POST | `/api/v1/auth/login` | `{ login, password }` | `{ user, csrf_token }` + Set-Cookie |
| POST | `/api/v1/auth/register` | `{ login, password }` | `{ user, csrf_token }` + Set-Cookie |
| POST | `/api/v1/auth/bootstrap` | `{ login, password, bootstrap_token }` | `{ user, csrf_token }` + Set-Cookie |
| POST | `/api/v1/auth/logout` | - | `{ ok: true }` |
| POST | `/api/v1/auth/change-password` | `{ current_password, new_password }` | `{ ok: true }` |

### 8.2 Sessions

| Method | Endpoint | Body | Response |
|--------|----------|------|----------|
| GET | `/api/v1/sessions` | - | `{ sessions: [SessionSummary] }` |
| POST | `/api/v1/sessions` | `{}` | `{ session: SessionDetail }` |
| GET | `/api/v1/sessions/:id` | - | `{ session: SessionDetail }` |
| PATCH | `/api/v1/sessions/:id` | `{ title }` | `{ session: SessionDetail }` |
| DELETE | `/api/v1/sessions/:id` | - | `{ ok: true }` |

### 8.3 Tasks

| Method | Endpoint | Body | Response |
|--------|----------|------|----------|
| GET | `/api/v1/sessions/:id/tasks` | - | `{ tasks: [TaskSummary] }` |
| POST | `/api/v1/sessions/:id/tasks` | `{ input_markdown }` | `{ task: TaskSummary }` |
| GET | `/api/v1/sessions/:id/tasks/:tid` | - | `{ task: TaskDetail }` |
| GET | `.../tasks/:tid/progress` | - | `{ task_id, status, progress, last_event_seq }` |
| GET | `.../tasks/:tid/events?after_seq=N&limit=N` | - | `{ events: [PersistedTaskEvent], last_seq, has_more }` |
| GET | `.../tasks/:tid/stream?after_seq=N` | - | SSE stream |
| PATCH | `.../tasks/:tid/input` | `{ input_markdown }` | `{ task: TaskSummary }` |
| POST | `.../tasks/:tid/resume` | `{ input_markdown }` | `{ task: TaskSummary }` |
| POST | `.../tasks/:tid/cancel` | - | `{ ok: true }` |

### 8.4 Key Data Types

```
SessionSummary { session_id, title, last_preview, active_task_id, last_task_status, created_at, updated_at }
SessionDetail  { session_id, title, active_task_id, last_task_status, created_at, updated_at }
TaskSummary    { task_id, status, input_markdown, input_edited_at, final_response_markdown, error_message, pending_user_input, last_event_seq, timestamps... }
TaskDetail     { ...TaskSummary + session_id, last_progress, last_event_seq }
TaskStatus     { Queued | Running | WaitingForUserInput | Completed | Failed | Cancelled | Interrupted }
TaskEventKind  { Thinking, TokenSnapshotUpdated, ToolCall, ToolResult, FileToSend, Continuation, Finished, Cancelling, Cancelled, Error, Reasoning, LoopDetected, Milestone, RuntimeCompactionStarted, RuntimeCompactionCompleted, RuntimeCompactionFailed, RuntimeCompactionSkipped, RepeatedCompactionWarning, HistoryRepairApplied, RateLimitRetrying, LlmRetrying, ProviderFailoverActivated, TodosUpdated }
PersistedTaskEvent { schema_version, task_id, session_id, user_id, seq, created_at, kind, summary, payload: Value, redacted, truncated }
ProgressSnapshot   { latest_token_snapshot, current_todos, last_compaction_status, llm_retry, ... }
```

---

## 9. Feature Inventory (what exists today)

### 9.1 Implemented features

- [x] Cookie-based auth with CSRF protection
- [x] User registration + bootstrap (first admin)
- [x] Session CRUD with search
- [x] Task creation, execution, cancellation
- [x] SSE streaming with reconnect, backfill, keepalive
- [x] Live progress display (token budget, compaction status, todos)
- [x] Inline agent activity: tool calls, tool results, reasoning, errors
- [x] Specialized tool cards: Shell, Web Search, Generic
- [x] Markdown rendering with sanitization + code copy
- [x] Task input editing (for terminal tasks)
- [x] Resume after user input
- [x] Context budget display (free tokens, flow, prompt, tools)
- [x] Settings page with password change + logout
- [x] Status badges for all task states
- [x] Empty states and error banners
- [x] Responsive layout (sidebar hides on small screens)
- [x] Session status dots (color-coded by last task status)
- [x] Todo list display in progress panel
- [x] Raw JSON inspector for tool outputs

### 9.2 Design constraints (non-goals per PRD)

- No pixel-perfect design system
- No dark/light theme toggle
- No complex animations
- No mobile app
- No file upload workflows
- No admin dashboard
- No approve/reject UI (YOLO mode)
- No SQL database

---

## 10. Key Constants

```
AUTH_COOKIE_NAME:          "oxide_web_session"
CSRF_HEADER_NAME:          "x-csrf-token"
SESSION_DEFAULT_TITLE:     "New session"
MAX_SESSION_TITLE_CHARS:   160
MAX_TASK_INPUT_CHARS:      65,536
TASK_PREVIEW_CHARS:        96
DEFAULT_TASK_EVENTS_LIMIT: 200
MAX_TASK_EVENTS_LIMIT:     500
AUTH_SESSION_TTL_SECS:     1,209,600 (14 days)
AUTH_RATE_LIMIT_WINDOW:    60s
AUTH_RATE_LIMIT_MAX:       5 failures
EVENT_SUMMARY_MAX_CHARS:   160
EVENT_PREVIEW_MAX_CHARS:   4,000
```

---

## 11. Color Application Map

| Element | Color | Token |
|---------|-------|-------|
| Page background | `#171717` | `--bg-root` |
| Sidebar background | `#171717` | `--bg-panel` |
| Input/tool card bg | `#212121` | `--bg-panel-soft` |
| Hover states | `#2f2f2f` | `--bg-panel-hover` |
| Primary text | `#ececec` | `--text-main` |
| Secondary text | `#8e8e93` | `--text-muted` |
| Timestamps/labels | `#6e6e73` | `--text-faint` |
| Links/primary action | `#10a37f` | `--accent-blue` |
| Running state | `#10a37f` + pulse animation | `--accent-blue` |
| Success/Completed | `#10a37f` | `--accent-green` |
| Warning/Waiting | `#d8a21e` | `--accent-yellow` |
| Error/Failed | `#ef4444` | `--accent-red` |
| Input border (default) | `#3a3a3a` | `--border-strong` |
| Dividers | `#2a2a2a` | `--border-subtle` |
| Input focus glow | `rgba(16,163,127,0.18)` | `--accent-glow` |

---

## 12. Interaction Patterns

- **Session delete**: Hover reveals "Del" button -> `window.confirm()` dialog -> API call -> remove from list
- **Task submission**: Ctrl+Enter or click "Run Agent" -> clear input -> create task -> start SSE stream
- **Task resume**: When `WaitingForUserInput`, button changes to "Resume", textarea submits to resume endpoint
- **Task cancel**: "Stop" button (only active when task is running) -> API cancel -> clear active task
- **Task input edit**: "Edit input" button on latest terminal task -> inline textarea with Save/Cancel -> PATCH API
- **Tool card expansion**: `<details>` element, auto-opened when running or failed or no stream content
- **Code copy**: Click "Copy" button next to code blocks -> clipboard API
- **SSE reconnect**: Up to 3 attempts, backfill on each reconnect, state indicator updates

---

## 13. File Inventory Summary

```
crates/oxide-agent-web-ui/
  Cargo.toml              # Dependencies: leptos, gloo-net, gloo-timers, wasm-bindgen, comrak, ammonia, serde_json
  Trunk.toml              # Build config for WASM
  index.html              # HTML shell for Trunk
  clippy.toml             # Lint config
  src/
    main.rs               # WASM entry
    app.rs                # Root component (108 lines)
    routes.rs             # Route enum (33 lines)
    components.rs         # Layout + shared components (97 lines)
    sessions.rs           # Session sidebar (225 lines)
    tasks.rs              # Task console + all card components (1482 lines)
    sse.rs                # SSE streaming client (461 lines)
    auth.rs               # Auth pages + context (353 lines)
    api.rs                # HTTP client (316 lines)
    markdown.rs           # Markdown rendering (127 lines)
    utils.rs              # Browser utilities (43 lines)
    styles.css            # Full design system (1824 lines)
  dist/                   # Compiled WASM output
    index.html
    oxide-agent-web-ui-*.wasm
    oxide-agent-web-ui-*.js
    styles-*.css

crates/oxide-agent-web-contracts/
  src/
    lib.rs                # Shared types re-export
    auth.rs               # Auth request/response types
    sessions.rs           # Session types
    tasks.rs              # Task types + TaskStatus + TaskEventKind
    events.rs             # PersistedTaskEvent + SseConnectionState
    error.rs              # ErrorEnvelope + ErrorCode
    config.rs             # PublicConfigResponse
```
