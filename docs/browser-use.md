# Browser Use Operations

Операторский runbook для self-hosted интеграции Browser Use в Oxide Agent.

Следующий архитектурный этап зафиксирован отдельно в [Browser Use Stage A](./browser-use-stage-a.md): там описан переход от bridge-side LLM env к inheritance route из Oxide Agent для `MiniMax`, `ZAI` и других основных provider-ов.

Следующий post-v1 decision slice зафиксирован в [Browser Use Post-v1 Decisions](./browser-use-post-v1.md): low-level browser surface отложен, а следующим implementation priority выбран topic-scoped persistent profile reuse.

## Что уже входит в rollout

- `browser_use` sidecar в `docker-compose.yml`
- `browser_use_bridge` HTTP service с endpoint-ами `GET /health`, `POST /sessions/run`, `GET /sessions/{id}`, `DELETE /sessions/{id}`
- Rust provider в `oxide-agent-core` с tool-ами:
  - `browser_use_run_task`
  - `browser_use_get_session`
  - `browser_use_close_session`
  - `browser_use_extract_content`
  - `browser_use_screenshot`
- регистрация tools в main agent, sub-agent и manager control plane
- manager alias-ы `browser` и `browser_use` для topic-level enable/disable

## Runtime Topology

- `oxide_agent` обращается к `browser_use` по `BROWSER_USE_URL`
- `browser_use` публикуется только на loopback `127.0.0.1:8002`
- browser state и session metadata сохраняются в volume `browser-use-data`
- reusable profile metadata и browser state теперь хранятся отдельно под `profiles/`, а не смешиваются с `sessions/` и `artifacts/`
- bridge уже поддерживает request-level `browser_llm_config` для нормализованного выбора LLM
- legacy env path через `BROWSER_USE_BRIDGE_LLM_PROVIDER` остается временным fallback
- Stage C уже прокидывает active Oxide route в bridge `browser_llm_config` для совместимых provider-ов
- Stage D передает inherited-route API key server-to-server через внутренний header, а не через request body
- Stage E вводит capability policy для text-only vs vision-capable routes
- Stage F делает route inheritance основным operator path в дефолтном `docker-compose` и добавляет runtime observability по `llm_source` / `vision_mode`
- Stage 1 reuse slice добавляет optional `reuse_profile` / `profile_id` в `browser_use_run_task` и отдельные profile records в bridge storage
- Stage 2 reuse wiring прокидывает hidden `profile_scope` из реального `context_key` и вводит quota на retained profiles per scope
- Stage 3 lifecycle cleanup теперь detaches reusable profiles на graceful shutdown bridge, auto-recovers orphaned `active` profiles после restart/crash и TTL-prune-ит старые idle/stale profiles до quota check
- Stage 1 dedicated browser route добавляет отдельный Oxide-side override для Browser Use, чтобы browser automation можно было держать на `zai / GLM-4.6V`, даже если main/sub-agent идут по другому route
- post-v1 decision slice фиксирует, что low-level browser actions пока не выводятся в основной tool surface; следующий приоритет - controlled profile reuse
- legacy env path остается fallback, когда route inheritance недоступен

## Capability Matrix

- `gemini` route считается vision-capable
- dedicated `zai` route с `GLM-4.6V` считается vision-capable для Browser Use
- `openrouter` route считается vision-capable только для моделей, которые выглядят мультимодальными по model id, например `gemini`, `gpt-4o`, `claude-3`, `vision`, `vl`, `pixtral`
- `minimax` и остальные `zai` route в текущем inheritance path считаются text-only route
- text-only route допустимы для summary/extraction/browsing задач
- для interactive UI задач Browser Use теперь возвращает warning о degraded mode
- для задач, явно требующих visual grounding, Browser Use завершает tool вызов понятной ошибкой до запуска sidecar session

## Важные переменные окружения

### В `oxide_agent`

- `BROWSER_USE_ENABLED=true`
- `BROWSER_USE_URL=http://127.0.0.1:8002`
- `BROWSER_USE_TIMEOUT_SECS=300`
- `BROWSER_USE_MAX_CONCURRENT=2`
- `BROWSER_USE_MODEL_ID=GLM-4.6V` - optional dedicated Browser Use route
- `BROWSER_USE_MODEL_PROVIDER=zai` - optional dedicated Browser Use provider

### В `browser_use` sidecar

Ниже перечислены fallback-переменные sidecar. Начиная со Stage C основной Rust provider уже сам прокидывает request-level `browser_llm_config` из активного Oxide route для `gemini`, `minimax`, `zai` и `openrouter`.

Начиная со Stage 1 dedicated browser route Rust provider сначала смотрит на `BROWSER_USE_MODEL_ID` / `BROWSER_USE_MODEL_PROVIDER`, и только если они не заданы, откатывается к текущему active Oxide route.

Начиная со Stage F дефолтный `docker-compose.yml` больше не прокидывает `BROWSER_USE_BRIDGE_LLM_PROVIDER` и `BROWSER_USE_BRIDGE_LLM_MODEL` в sidecar. Если legacy env path все еще нужен, его надо включать через compose override или отдельное runtime env для контейнера `browser_use`.

- `BROWSER_USE_BRIDGE_HOST=0.0.0.0`
- `BROWSER_USE_BRIDGE_PORT=8000`
- `BROWSER_USE_BRIDGE_DATA_DIR=/data`
- `BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS=120`
- `BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS=300`
- `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS=2`
- `BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE=3`
- `BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS=604800` - idle/stale profile TTL; `0` disables pruning
- `BROWSER_USE_BRIDGE_LLM_PROVIDER=google|anthropic|browser_use`
- `BROWSER_USE_BRIDGE_LLM_MODEL=<optional-model-id>`

Для inherited route отдельные sidecar env c ключами `MINIMAX_API_KEY`, `ZAI_API_KEY`, `OPENROUTER_API_KEY` больше не обязательны в дефолтном compose: Oxide Agent отправляет нужный key во внутреннем запросе к bridge через `X-Oxide-Browser-Llm-Api-Key`.

### Upstream credentials

Нужно передать API key для выбранного bridge LLM provider:

- `GEMINI_API_KEY` для `BROWSER_USE_BRIDGE_LLM_PROVIDER=google`
- `ANTHROPIC_API_KEY` для `BROWSER_USE_BRIDGE_LLM_PROVIDER=anthropic`
- provider-specific key для `BROWSER_USE_BRIDGE_LLM_PROVIDER=browser_use`, если этот режим используется

Если используется ручной request-level `browser_llm_config` с `api_key_ref=env:...`, соответствующий env должен существовать внутри контейнера `browser_use`.

Минимально важные случаи:

- `MINIMAX_API_KEY` в `oxide_agent` для inherited route `provider=minimax`
- `ZAI_API_KEY` в `oxide_agent` для dedicated Browser Use route `provider=zai` или inherited route `provider=zai`
- `OPENROUTER_API_KEY` в `oxide_agent` для inherited route `provider=openrouter`

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
  "import_error": null,
  "preferred_browser_llm_source": "request_browser_llm_config",
  "legacy_env_fallback_configured": false
}
```

Полезные поля в `/health`:

- `preferred_browser_llm_source` показывает, что primary path идет через request-level `browser_llm_config`
- `legacy_env_fallback_configured` показывает, включен ли старый env fallback на этом sidecar
- `supported_inherited_route_providers` показывает, какие route provider-ы Rust provider умеет прокидывать автоматически
- `supported_legacy_env_providers` показывает, какие bridge-local adapter-ы еще остаются для fallback-сценариев
- `profile_idle_ttl_secs` показывает, через сколько bridge auto-prune-ит idle/stale profiles
- `orphan_profile_recovery_supported` показывает, что bridge умеет self-heal-ить `active` profiles, оставшиеся после рестарта

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
- `browser_use_extract_content`
- `browser_use_screenshot`

Если нужен точечный контроль, можно включать и блокировать отдельные инструменты по именам.

## Быстрые проверки после запуска

1. Убедиться, что compose healthcheck зеленый для `browser_use`.
2. Убедиться, что `BROWSER_USE_ENABLED=true` и `BROWSER_USE_URL` видны контейнеру `oxide_agent`.
3. Для legacy env path убедиться, что bridge-side LLM provider и его API key переданы в контейнер `browser_use`.
4. Для Stage C inheritance path убедиться, что активный route агента использует совместимый provider: `gemini`, `minimax`, `zai` или `openrouter`.
5. Для inherited route убедиться, что нужный provider key задан в `oxide_agent`, а не только в `browser_use` sidecar.
6. Если используется fallback/request-level path вручную, убедиться, что `browser_llm_config` содержит совместимый provider/model и корректный `api_key_ref`.
7. Через manager `topic_agent_tools_get` проверить, что в `provider_statuses` появился `browser_use`.
8. Выполнить smoke task через `browser_use_run_task` с простой страницей и коротким timeout.
9. Если нужен reuse, запустить `browser_use_run_task` с `reuse_profile=true` и сохранить возвращенный `profile_id`.
10. При reuse убедиться, что вызов идет из того же topic/context: Stage 2 теперь шьет hidden `profile_scope` из runtime context и не даст reuse-ить profile из другого topic.
11. После restart bridge не очищать metadata вручную: Stage 3 сам переведет orphaned `active` profile в recoverable state при следующем reuse.
12. В ответе `browser_use_run_task` или `GET /sessions/{id}` проверить поля `llm_source`, `llm_provider`, `llm_transport`, `vision_mode`, `profile_id`, `profile_scope`, `profile_status` и `profile_attached`, чтобы убедиться, что реально используется inherited route и при необходимости привязан reusable profile.

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

- активный inherited route использует пока неподдерживаемый provider, например `groq`, `mistral` или `nvidia`
- для inherited route отсутствует нужный provider key в `oxide_agent`, поэтому Rust provider не может передать secret в bridge
- inherited route text-only, а задача явно просит visual analysis, screenshot-like reasoning или оценку layout/colors
- `profile_id` пытаются reuse-ить из другого topic/context, и bridge режет запрос по injected `profile_scope`
- в текущем topic/context уже достигнут quota `BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE`, и bridge не создает новый retained profile
- quota не освобождается так быстро, как ожидается: проверить `BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS` и помнить, что prune применяется к idle/stale profiles, а не к активно прикрепленным
- не задан `BROWSER_USE_BRIDGE_LLM_PROVIDER` для legacy env path
- не передан API key для выбранного provider
- `browser_llm_config.api_key_ref` указывает на отсутствующий env
- bridge не может создать совместимый Browser Use LLM wrapper для выбранного transport-а

Если в ответе видно `llm_source=legacy_env`, хотя ожидался inheritance path, это операторский сигнал, что route context не был передан или запрос шел вне обычного agent execution path.

После перехода на Stage A основным классом ошибок станет уже не отсутствие bridge env, а несовместимость inherited route или его credentials.

### Session создается, но браузерные задачи нестабильны

Что проверить:

- хватает ли `shm_size` для Chromium
- не слишком ли низкий `timeout_secs`
- нет ли перегруза по `BROWSER_USE_MAX_CONCURRENT` или `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS`

## Рекомендуемый v1 usage pattern

- использовать Browser Use для задач уровня “открой сайт, пройди пару шагов, собери summary”
- после `browser_use_run_task` можно дочитать страницу через `browser_use_extract_content` или снять PNG через `browser_use_screenshot`
- если нужен controlled reuse login/cookie state между задачами, сначала вызвать `browser_use_run_task` с `reuse_profile=true`, а затем переиспользовать возвращенный `profile_id` в следующем `browser_use_run_task`
- не расширять без необходимости tool surface до raw click/type/eval action-ов; это отложено отдельным post-v1 decision slice
- не рассматривать его как замену `searxng` или `crawl4ai`
- закрывать долгоживущие сессии через `browser_use_close_session`, если reuse больше не нужен
- не держать бесконечно много idle profiles в одном topic: Stage 3 чистит их по TTL, поэтому для реально долгого reuse TTL нужно держать осознанно настроенным
- включать Browser Use topic-by-topic, а не глобально для всех профилей без необходимости
- рассчитывать на то, что persistent profile reuse теперь topic/context-scoped: reuse одного `profile_id` из другого topic будет отвергнут bridge-ом
