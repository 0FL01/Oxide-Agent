# Browser Use Operations

Операторский runbook для self-hosted интеграции Browser Use в Oxide Agent.

Следующий архитектурный этап зафиксирован отдельно в [Browser Use Stage A](./browser-use-stage-a.md): там описан переход от bridge-side LLM env к inheritance route из Oxide Agent для `MiniMax`, `ZAI` и других основных provider-ов.

## Что уже входит в rollout

- `browser_use` sidecar в `docker-compose.yml`
- `browser_use_bridge` HTTP service с endpoint-ами `GET /health`, `POST /sessions/run`, `GET /sessions/{id}`, `DELETE /sessions/{id}`
- Rust provider в `oxide-agent-core` с tool-ами:
  - `browser_use_run_task`
  - `browser_use_get_session`
  - `browser_use_close_session`
- регистрация tools в main agent, sub-agent и manager control plane
- manager alias-ы `browser` и `browser_use` для topic-level enable/disable

## Runtime Topology

- `oxide_agent` обращается к `browser_use` по `BROWSER_USE_URL`
- `browser_use` публикуется только на loopback `127.0.0.1:8002`
- browser state и session metadata сохраняются в volume `browser-use-data`
- bridge уже поддерживает request-level `browser_llm_config` для нормализованного выбора LLM
- legacy env path через `BROWSER_USE_BRIDGE_LLM_PROVIDER` остается временным fallback
- Stage A фиксирует целевую модель через route inheritance из Oxide Agent

## Важные переменные окружения

### В `oxide_agent`

- `BROWSER_USE_ENABLED=true`
- `BROWSER_USE_URL=http://127.0.0.1:8002`
- `BROWSER_USE_TIMEOUT_SECS=300`
- `BROWSER_USE_MAX_CONCURRENT=2`

### В `browser_use` sidecar

Ниже перечислены fallback-переменные sidecar. Начиная со Stage B bridge также умеет принимать request-level `browser_llm_config`, в том числе для `minimax` и `zai`.

- `BROWSER_USE_BRIDGE_HOST=0.0.0.0`
- `BROWSER_USE_BRIDGE_PORT=8000`
- `BROWSER_USE_BRIDGE_DATA_DIR=/data`
- `BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS=120`
- `BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS=300`
- `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS=2`
- `BROWSER_USE_BRIDGE_LLM_PROVIDER=google|anthropic|browser_use`
- `BROWSER_USE_BRIDGE_LLM_MODEL=<optional-model-id>`

### Upstream credentials

Нужно передать API key для выбранного bridge LLM provider:

- `GEMINI_API_KEY` для `BROWSER_USE_BRIDGE_LLM_PROVIDER=google`
- `ANTHROPIC_API_KEY` для `BROWSER_USE_BRIDGE_LLM_PROVIDER=anthropic`
- provider-specific key для `BROWSER_USE_BRIDGE_LLM_PROVIDER=browser_use`, если этот режим используется

Если используется request-level `browser_llm_config` с `api_key_ref=env:...`, соответствующий env должен существовать внутри контейнера `browser_use`.

Минимально важные случаи:

- `MINIMAX_API_KEY` для `provider=minimax`
- `ZAI_API_KEY` для `provider=zai`

Если ключа нет, bridge поднимется, но `browser_use_run_task` будет завершаться ошибкой на этапе создания LLM.

## Сборка и запуск

После Stage 8 основной Docker image собирается с feature-флагом `oxide-agent-core/browser_use`, поэтому отдельная ручная сборка feature больше не нужна при запуске через основной `Dockerfile`.

Стандартный запуск:

```bash
docker compose up --build -d browser_use oxide_agent
```

Проверка статуса:

```bash
docker compose ps browser_use oxide_agent
curl -f http://127.0.0.1:8002/health
```

Ожидаемый healthy-ответ bridge:

```json
{
  "status": "ok",
  "browser_use_available": true,
  "import_error": null
}
```

## Topic-Agent UX

Browser Use не включается через alias `search`. Для него есть отдельная provider-group `browser_use`.

В manager control plane можно включать Browser Use так:

```json
{
  "topic_id": "topic-a",
  "tools": ["browser"]
}
```

или так:

```json
{
  "topic_id": "topic-a",
  "tools": ["browser_use"]
}
```

Это раскрывается в:

- `browser_use_run_task`
- `browser_use_get_session`
- `browser_use_close_session`

Если нужен точечный контроль, можно включать и блокировать отдельные инструменты по именам.

## Быстрые проверки после запуска

1. Убедиться, что compose healthcheck зеленый для `browser_use`.
2. Убедиться, что `BROWSER_USE_ENABLED=true` и `BROWSER_USE_URL` видны контейнеру `oxide_agent`.
3. Для legacy env path убедиться, что bridge-side LLM provider и его API key переданы в контейнер `browser_use`.
4. Для Stage B request-level path убедиться, что `browser_llm_config` содержит совместимый provider/model и корректный `api_key_ref`.
5. Для следующего inheritance path сверяться с `Browser Use Stage A`, а не вводить отдельную модель вручную без необходимости.
6. Через manager `topic_agent_tools_get` проверить, что в `provider_statuses` появился `browser_use`.
7. Выполнить smoke task через `browser_use_run_task` с простой страницей и коротким timeout.

## Типичные сбои

### `/health` возвращает `503`

Обычно это означает, что Python runtime sidecar не смог импортировать `browser_use` или его зависимости.

Что проверить:

- логи контейнера `browser_use`
- успешность build-а image
- наличие Chromium и Python dependencies в sidecar image

### Tool не появляется у агента

Что проверить:

- контейнер `oxide_agent` пересобран после Stage 8
- feature `oxide-agent-core/browser_use` включен в основном `Dockerfile`
- `BROWSER_USE_ENABLED=true`
- `BROWSER_USE_URL` непустой

### `browser_use_run_task` падает сразу

Частые причины:

- не задан `BROWSER_USE_BRIDGE_LLM_PROVIDER` для legacy env path
- не передан API key для выбранного provider
- `browser_llm_config.api_key_ref` указывает на отсутствующий env
- bridge не может создать совместимый Browser Use LLM wrapper для выбранного transport-а

После перехода на Stage A основным классом ошибок станет уже не отсутствие bridge env, а несовместимость inherited route или его credentials.

### Session создается, но браузерные задачи нестабильны

Что проверить:

- хватает ли `shm_size` для Chromium
- не слишком ли низкий `timeout_secs`
- нет ли перегруза по `BROWSER_USE_MAX_CONCURRENT` или `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS`

## Рекомендуемый v1 usage pattern

- использовать Browser Use для задач уровня “открой сайт, пройди пару шагов, собери summary”
- не рассматривать его как замену `searxng` или `crawl4ai`
- закрывать долгоживущие сессии через `browser_use_close_session`, если reuse больше не нужен
- включать Browser Use topic-by-topic, а не глобально для всех профилей без необходимости
