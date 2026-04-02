# Stack Logs Stage 0

Approved Stage 0 contract для нового инструмента инспекции логов docker-compose стека в Oxide Agent.

Stage 0 фиксирует surface API, access policy и stack selection. Реализация broker contract, Docker-side collection и filtering остаются следующими стадиями.

## Цель

- дать top-level агенту безопасный способ читать логи всего compose-стека без прямого доступа `oxide_agent` к Docker socket
- вернуть bounded JSON вместо сырых log dump-ов, чтобы не раздувать memory и progress UI
- поддержать выбор сервисов, time window и постраничное line-by-line чтение через cursor

## Approved Decisions

### Tool surface

Stage 0 фиксирует два tool-а вместо одного перегруженного инструмента:

- `stack_logs_list_sources`
- `stack_logs_fetch`

Причина: discovery и чтение логов имеют разную семантику, разные лимиты и разный UX для агента.

### Provider and alias

- provider module name: `stack_logs`
- manager/tool catalog alias: `stack_logs`
- человеко-ориентированное описание в UI и docs: `Stack Logs`

Название `debug-tool` в кодовую surface не выносится, чтобы не смешивать log inspection с другими debug use-case-ами.

### Access policy

- tools доступны только top-level агентам
- tools не доступны sub-agent-ам в v1
- topic-level tool management поддерживается через alias `stack_logs`
- для topic agent profiles tools считаются blocked by default и должны включаться явно

Причина: инструмент читает operational logs всего compose-стека и должен оставаться явным opt-in capability.

### Transport and execution path

- tool provider живет в `crates/oxide-agent-core/src/agent/providers/stack_logs.rs`
- provider не обращается к Docker напрямую из `oxide_agent`
- provider ходит в Docker-capable сторону только через `sandboxd` broker
- provider не запускает внутренний sub-agent
- LLM-based summarization внутри provider не допускается

Причина: текущий runtime не дает `oxide_agent` доступа к Docker socket, а внутренний sub-agent внутри tool provider усложнил бы failure model и observability.

### Stack selector

Stage 0 фиксирует один основной selector и один deployment override:

- primary selector: Docker label `com.docker.compose.project`
- optional env override: `STACK_LOGS_PROJECT`

Правила:

- если задан `STACK_LOGS_PROJECT`, broker ищет контейнеры с `com.docker.compose.project=<value>`
- если `STACK_LOGS_PROJECT` не задан, broker использует compose project label текущего runtime deployment
- arbitrary label selector в Stage 0 не вводится

Причина: этого достаточно для стандартного compose deployment, а surface остается минимальной.

## Tool contracts

### `stack_logs_list_sources`

Назначение: перечислить доступные сервисы и контейнеры выбранного compose-стека.

Arguments:

```json
{
  "services": ["oxide_agent", "browser_use"],
  "include_stopped": false
}
```

Notes:

- `services` optional; если не передан, возвращаются все сервисы выбранного стека
- `include_stopped` default = `false`

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

Назначение: получить нормализованный bounded log stream по window или cursor.

Arguments:

```json
{
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

Notes:

- `services` optional; по умолчанию читаются все сервисы выбранного стека
- `since` and `until` use RFC3339 timestamps
- `cursor` optional и используется для продолжения чтения без дублей
- `max_entries` default = `200`, hard max = `500`
- `include_noise` default = `false`
- `include_stderr` default = `true`

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
      "message": "provider failover activated after repeated 429 responses"
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

## Cursor contract

Stage 0 фиксирует стабильный cursor key:

- `ts`
- `service`
- `stream`
- `ordinal`

Merge order across containers:

1. `ts`
2. `service`
3. `stream`
4. `ordinal`

Причина: timestamp alone не гарантирует стабильную пагинацию при совпадающих log timestamps.

## Noise policy

Stage 0 фиксирует conservative filtering only. При `include_noise=false` допускается подавление только следующих классов:

- empty lines
- exact duplicate bursts
- known health/readiness probe chatter

Дополнительно:

- suppressed entries не теряются молча; tool всегда возвращает `suppressed` counters
- semantic summarization, clustering и LLM rewrite в v1 не допускаются

## Non-goals for Stage 0

- broker wire implementation
- Docker log collection implementation
- arbitrary regex filtering
- pattern search API
- internal sub-agent orchestration inside provider
- returning raw unbounded logs

## Exit criteria for Stage 0

- имена tool-ов и provider alias зафиксированы
- access policy зафиксирована
- stack selector зафиксирован
- JSON request/response shape зафиксирован
- cursor and noise policy зафиксированы без перехода к Stage 1 implementation
