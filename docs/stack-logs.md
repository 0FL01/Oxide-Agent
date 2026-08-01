# Stack Logs

Reference for the Stack Logs tool: compose-stack log discovery and bounded retrieval for top-level agents.

## Purpose

- Give the top-level agent a safe way to read logs of the entire Docker Compose stack without direct Docker socket access from the `oxide_agent` process.
- Return bounded JSON instead of raw log dumps, so memory and progress UI are not inflated.
- Support service selection, time windows, and cursor-based line-by-line pagination.

## Tools

Two tools with separate semantics:

| Tool | Purpose |
|------|---------|
| `stack_logs_list_sources` | List available services and containers in the selected compose stack. |
| `stack_logs_fetch` | Fetch a bounded, normalized log stream by time window or cursor. |

### `stack_logs_list_sources`

Arguments:

```json
{
  "selector": {
    "compose_project": "oxide-agent"
  },
  "services": ["oxide_agent", "crw"],
  "include_stopped": false
}
```

- `selector` — optional; `compose_project` overrides the stack selection for this call.
- `services` — optional; when omitted, all services in the selected stack are returned.
- `include_stopped` — default `false`.

Response:

```json
{
  "stack_selector": {
    "compose_project": "oxide-agent"
  },
  "containers": [
    {
      "service": "oxide_agent",
      "container_name": "oxide_agent",
      "container_id": "abc123def456",
      "state": "running",
      "started_at": "2026-04-02T10:11:12Z"
    }
  ]
}
```

### `stack_logs_fetch`

Arguments:

```json
{
  "selector": {
    "compose_project": "oxide-agent"
  },
  "services": ["oxide_agent", "sandboxd"],
  "since": "2026-04-02T10:00:00Z",
  "until": "2026-04-02T10:10:00Z",
  "cursor": {
    "ts": "2026-04-02T10:03:04.500Z",
    "service": "oxide_agent",
    "stream": "stdout",
    "ordinal": 17
  },
  "max_entries": 200,
  "include_noise": false,
  "include_stderr": true
}
```

- `selector` — optional; same as in `list_sources`.
- `services` — optional; when omitted, all services in the selected stack are read.
- `since` / `until` — RFC3339 timestamps. If both are set, `since` must be ≤ `until`.
- `cursor` — optional; returned by a previous `fetch` call for continuation without duplicates.
- `max_entries` — default `200`, hard max `500`. Values of `0` are clamped to `1` with a warning.
- `include_noise` — default `false`.
- `include_stderr` — default `true`.

Response:

```json
{
  "window": {
    "since": "2026-04-02T10:00:00Z",
    "until": "2026-04-02T10:10:00Z"
  },
  "entries": [
    {
      "ts": "2026-04-02T10:03:04.500Z",
      "service": "oxide_agent",
      "container_name": "oxide_agent",
      "stream": "stdout",
      "ordinal": 17,
      "message": "retrying LLM request after rate limit"
    }
  ],
  "suppressed": [
    {
      "reason": "exact_duplicate_burst",
      "count": 12
    }
  ],
  "truncated": false,
  "next_cursor": {
    "ts": "2026-04-02T10:03:04.500Z",
    "service": "oxide_agent",
    "stream": "stdout",
    "ordinal": 18
  },
  "warnings": []
}
```

## Stack selector

Three-level resolution, in priority order:

1. **Per-request override** — `selector.compose_project` in the tool arguments.
2. **Environment variable** — `STACK_LOGS_PROJECT`.
3. **Runtime detection** — the sandboxd container's `com.docker.compose.project` Docker label, looked up via `HOSTNAME` → `inspect_container`.

If none resolve, the call fails with an error (see Error handling).

Docker Compose wiring sets `STACK_LOGS_PROJECT=${COMPOSE_PROJECT_NAME:-oxide-agent}` in `docker-compose.yml`.

Arbitrary label selectors are not supported.

## Cursor contract

Cursor key: `ts`, `service`, `stream`, `ordinal` — all required.

Merge order for cross-container pagination:

1. `ts`
2. `service`
3. `stream`
4. `ordinal`

Timestamp alone does not guarantee stable pagination when log timestamps collide. Ordinals are assigned per `(service, stream)` after sorting and noise filtering, making the cursor unambiguous.

## Noise policy

When `include_noise=false`, three classes are suppressed:

- **Empty lines** — `reason: "empty_line"`.
- **Health/readiness probe chatter** — messages mentioning endpoints (`/health`, `/healthz`, `/ready`, `/readyz`, `/readiness`, `/live`, `/livez`) alongside keywords (`get`, `head`, `healthcheck`, `kube-probe`, `probe`). `reason: "health_probe_chatter"`.
- **Exact duplicate bursts** — same `service`, `container_name`, `stream`, and `message` within 1 second of the previous kept entry. `reason: "exact_duplicate_burst"`.

Suppressed entries are never lost silently: the tool always returns `suppressed` counters with reasons and counts. Semantic summarization, clustering, and LLM rewrite are not performed.

## Processing pipeline

`fetch_stack_logs` executes the following stages:

1. **Validate** — reject inverted time windows.
2. **Discover sources** — list containers filtered by `com.docker.compose.project=<resolved>`, enrich via `inspect_container`, sort by `service → container_name → container_id`.
3. **Collect per source** — bollard `docker.logs` with `follow=false`, `timestamps=true`, `tail=max_entries`, `stdout=true`, `stderr=include_stderr`, `since`/`until` bounds. Partial lines are buffered until newline; RFC3339 timestamp prefixes are parsed. Unparsable lines are counted and surfaced as warnings.
4. **Sort** — `ts → service → stream → container_name → ordinal`.
5. **Noise filter** — suppress the three noise classes (unless `include_noise=true`).
6. **Assign ordinals** — sequential per `(service, stream)`.
7. **Cursor filter** — keep entries strictly after the cursor.
8. **Paginate** — truncate to `max_entries`; if truncated, set `truncated=true` and `next_cursor` from the last returned entry.

## Access policy

- **Top-level agents** — full access.
- **Sub-agents** — blocked. Both tool names are in `BLOCKED_SUB_AGENT_TOOLS` (`delegation.rs`).
- **Topic agents** — blocked by default. Both tool names are in `TOPIC_AGENT_DEFAULT_BLOCKED_TOOLS` (`profile.rs`). Can be enabled explicitly through the manager control plane using the provider alias `"stack_logs"` or `"logs"`, which expands to both tool names (`agent_controls.rs`).

Prompt composer exposes a guidance group under alias `"stack_logs"` instructing the agent to call `list_sources` before `fetch`.

## Feature gating and module registry

| Item | Value |
|------|-------|
| Module ID | `tool/stack-logs` |
| Cargo feature | `tool-stack-logs` (also pulls in `sandbox-backend-sandboxd-client`) |
| cfg alias | `oxide_module_tool_stack_logs` |
| Capability provides | `tool/stack-logs` |
| Capability requires | `sandbox-backend/*/diagnostics` |
| Profiles | `full` only |

Source of truth: `crates/oxide-agent-core/module_registry.toml`. Compiled manifest in `crates/oxide-agent-core/src/capabilities/compiled.rs`.

## Architecture

```
Agent
  └─ StackLogsProvider (stack_logs.rs)
       └─ SandboxDiagnosticsRuntime (diagnostics.rs)
            └─ SandboxManager (broker.rs, client side)
                 └─ Unix socket ── sandboxd daemon (broker.rs, server side)
                      └─ DockerSandboxManager (manager.rs)
                           └─ bollard → Docker API
```

- The provider lives in `crates/oxide-agent-core/src/agent/providers/stack_logs.rs`.
- The provider never touches Docker directly. All Docker access goes through the sandboxd broker over a Unix socket.
- Broker wire: `SandboxBrokerRequest::ListStackLogSources` / `FetchStackLogs` → `SandboxBrokerResponse::StackLogSources` / `StackLogs`, serialized with bincode.
- The daemon (`DockerSandboxManager`) owns direct Docker access and performs container listing, log streaming, and all processing pipeline stages.
- Both tools share a single `Mutex` execution lock, serializing calls within one agent process.
- No internal sub-agent. No LLM summarization inside the provider.
- No persistence: all output is computed live from Docker logs on each call.

## Error handling

The provider does not fail tool calls on backend errors. Instead, it returns a JSON object:

```json
{ "error": "Unable to resolve compose project for stack log discovery; set STACK_LOGS_PROJECT or run sandboxd inside a Docker Compose deployment" }
```

Key error conditions:

| Condition | Message |
|-----------|---------|
| Unresolvable compose project | `Unable to resolve compose project for stack log discovery; set STACK_LOGS_PROJECT or run sandboxd inside a Docker Compose deployment` |
| `HOSTNAME` unavailable for runtime detection | `Unable to resolve compose project for stack log discovery automatically: HOSTNAME is unavailable; set STACK_LOGS_PROJECT` |
| Container missing compose label | `Unable to resolve compose project for stack log discovery automatically: current sandboxd container is missing label '...'; set STACK_LOGS_PROJECT` |
| Inverted time window | `Invalid stack log time window: 'since' must be earlier than or equal to 'until'` |
| All sources failed, no entries | `Failed to fetch logs for {service} ({container_name}): {error}` (joined with `; `) |

Warnings (non-fatal, returned in the `warnings` array):

- `max_entries=0` clamped to `1`: `Requested max_entries=0 is invalid; using 1`
- `max_entries>500` clamped to `500`: `Requested max_entries={n} exceeds hard limit 500; using 500`
- Unparsable log lines: `Skipped {n} unparsable timestamped log lines from {service} ({container_name})`
- Per-source fetch failures (when at least some entries were still collected from other sources)

## Configuration

| Variable | Purpose |
|----------|---------|
| `STACK_LOGS_PROJECT` | Optional compose project name override. Set in Docker Compose files to `${COMPOSE_PROJECT_NAME:-oxide-agent}`. |

No YAML config keys. Stack selection is purely env + per-request `selector.compose_project`.

## Testing

| Group | Location |
|-------|----------|
| Provider | `crates/oxide-agent-core/src/agent/providers/stack_logs.rs` (`mod tests`) |
| Manager unit | `crates/oxide-agent-core/src/sandbox/manager.rs` (`mod tests`) |
| Broker roundtrip | `crates/oxide-agent-core/src/sandbox/broker.rs` (`mod tests`) |
| Control-plane alias | `crates/oxide-agent-core/src/agent/providers/manager_control_plane/agent_controls.rs`, `tests/agent_controls.rs` |
| Profile blocklist | `crates/oxide-agent-core/src/agent/profile.rs` |
| Snapshot | `crates/oxide-agent-core/tests/snapshots/modular_registry_snapshots__modular_registry_snapshot@profile-full.snap` |

## Non-goals

- Arbitrary regex filtering.
- Pattern search API.
- Internal sub-agent orchestration inside the provider.
- Returning raw unbounded logs.
