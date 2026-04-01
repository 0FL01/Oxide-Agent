# Browser Use Stage 0

Этот документ фиксирует Stage 0 для интеграции `browser-use` в Oxide Agent.

## Решение

- `browser-use` интегрируется как отдельный self-hosted sidecar `browser_use` в `docker-compose`.
- Основной контейнер `oxide_agent` не получает Python/Chromium runtime внутрь себя.
- Взаимодействие между Oxide Agent и `browser-use` идет через тонкий HTTP bridge `browser_use_bridge`.
- orchestration и планирование действий остаются в Oxide Agent.
- `browser-use` используется как browser automation engine, а не как второй полноценный agent runtime.

## Почему выбран HTTP bridge

- Текущий self-hosted паттерн проекта уже построен вокруг sidecar-сервисов вроде `crawl4ai` и `searxng`.
- Текущие MCP-интеграции в core используют только `child-process` transport, а не remote HTTP MCP.
- Browser Use OSS официально дает Python library, CLI и локальный `stdio` MCP, но не стабильный OSS HTTP sidecar-контракт уровня `crawl4ai`.
- Отдельный bridge позволяет изолировать Python/browser runtime, сохранить текущую операционную модель и ограничить tool surface.

## Границы v1

В Stage 0 зафиксировано, что в первый релиз не входят:

- Browser Use Cloud API
- remote HTTP MCP от Browser Use Cloud
- прямой `stdio` MCP запуск внутри `oxide_agent`
- полный low-level browser action surface (`click`, `type`, `switch_tab` и т.д.)
- перенос Chromium/Playwright/Python зависимостей в Rust-контейнер агента

## Tool Contract v1

Стартовый tool surface должен быть компактным и высокоуровневым:

- `browser_use_run_task`
- `browser_use_get_session`
- `browser_use_close_session`

### `browser_use_run_task`

Назначение: выполнить ограниченную браузерную задачу через Browser Use и вернуть итог вместе с метаданными сессии.

Минимальные аргументы v1:

- `task: string` — инструкция для браузерного исполнения
- `start_url: string | null` — начальная страница, если пользователь ее явно задал
- `session_id: string | null` — reuse существующей сессии, если нужно продолжение
- `timeout_secs: integer | null` — override в пределах серверного лимита

Ожидаемый результат:

- `session_id`
- `status` (`running`, `completed`, `failed`)
- `final_url`
- `summary`
- `artifacts` (например, скриншоты или извлеченный текст, если bridge их вернул)
- `error` при неуспехе

### `browser_use_get_session`

Назначение: получить текущее состояние уже созданной браузерной сессии.

Минимальные аргументы v1:

- `session_id: string`

Ожидаемый результат:

- `session_id`
- `status`
- `current_url`
- `summary`
- `last_error`

### `browser_use_close_session`

Назначение: завершить браузерную сессию и освободить ресурсы.

Минимальные аргументы v1:

- `session_id: string`

Ожидаемый результат:

- `session_id`
- `closed: boolean`
- `status`

## HTTP Bridge Contract v1

Bridge предоставляет минимальный HTTP API:

- `GET /health`
- `POST /sessions/run`
- `GET /sessions/{id}`
- `DELETE /sessions/{id}`

### `GET /health`

Возвращает liveness/readiness sidecar-а.

Минимальный ответ:

```json
{
  "status": "ok"
}
```

### `POST /sessions/run`

Request body v1:

```json
{
  "task": "Open the docs site and summarize the main page",
  "start_url": "https://docs.example.com",
  "session_id": null,
  "timeout_secs": 120
}
```

Response body v1:

```json
{
  "session_id": "session_123",
  "status": "completed",
  "final_url": "https://docs.example.com/",
  "summary": "Main page loaded and summarized.",
  "artifacts": [],
  "error": null
}
```

### `GET /sessions/{id}`

Response body v1:

```json
{
  "session_id": "session_123",
  "status": "completed",
  "current_url": "https://docs.example.com/",
  "summary": "Main page loaded and summarized.",
  "last_error": null
}
```

### `DELETE /sessions/{id}`

Response body v1:

```json
{
  "session_id": "session_123",
  "closed": true,
  "status": "closed"
}
```

## Compose Contract v1

В `docker-compose.yml` добавляется сервис `browser_use` со следующими свойствами:

- loopback-only port publish
- healthcheck
- volume для browser/session state
- `shm_size` для Chromium
- restart policy
- `depends_on` из `oxide_agent`

`oxide_agent` получает env vars:

- `BROWSER_USE_ENABLED=true`
- `BROWSER_USE_URL=http://127.0.0.1:<port>`

## Provider Contract v1

В `oxide-agent-core` новый provider должен повторять operational pattern `crawl4ai`:

- отдельный provider module
- `reqwest` client
- timeout/retry/backoff
- cancellation-aware execution
- feature/runtime registration через env toggle
- отдельная регистрация для sub-agent с shared concurrency limit

## Policy Decisions

- Browser Use считается тяжелым browser automation tool, а не обычным search tool.
- В v1 tool surface умышленно ограничен тремя инструментами.
- Встроенный agent mode Browser Use не используется как основной orchestration layer.
- Расширение surface area допускается только после стабилизации bridge и resource envelope.

## Acceptance Criteria

Stage 0 считается завершенным, если:

- в репозитории зафиксирован выбранный архитектурный path
- явно описано, почему выбран HTTP bridge, а не прямой `stdio` MCP в `oxide_agent`
- зафиксирован минимальный tool surface v1
- зафиксирован минимальный HTTP contract bridge
- зафиксирован compose/runtime contract для self-hosted развертывания
- зафиксированы исключения и non-goals первого релиза
