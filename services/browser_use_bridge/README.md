# Browser Use Bridge

Минимальный HTTP bridge для интеграции `browser-use` в Oxide Agent.

## HTTP API

- `GET /health`
- `POST /sessions/run`
- `GET /sessions/{id}`
- `DELETE /sessions/{id}`
- `POST /sessions/{id}/extract_content`
- `POST /sessions/{id}/screenshot`

`POST /sessions/run` поддерживает два режима выбора LLM:

- request-level `browser_llm_config`, который уже используется Rust provider-ом для Stage C route inheritance и является основным режимом
- legacy fallback через `BROWSER_USE_BRIDGE_LLM_PROVIDER` / `BROWSER_USE_BRIDGE_LLM_MODEL`

Дополнительно `POST /sessions/run` теперь принимает optional `execution_mode`:

- `autonomous` - обычный full browse run
- `navigation_only` - более узкий bridge-side mode для steering-задач, где финальный screenshot/extract должен делаться follow-up tool-ами

Минимальный reuse slice добавляет в `POST /sessions/run` optional hints:

- `reuse_profile=true` - создать reusable profile и привязать его к новой browser session
- `profile_id` - reuse уже существующего profile

Начиная со Stage 2 основной Rust provider уже прокидывает hidden `profile_scope` из реального topic/context runtime. Прямые/manual bridge вызовы без этого поля по-прежнему fallback-ятся в `bridge_local`.

## Environment

- `BROWSER_USE_BRIDGE_HOST` - bind host, default `0.0.0.0`
- `BROWSER_USE_BRIDGE_PORT` - bind port, default `8000`
- `BROWSER_USE_BRIDGE_DATA_DIR` - data dir, default `/tmp/browser-use-bridge`
- `BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS` - default run timeout, default `120`
- `BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS` - max allowed timeout override, default `300`
- `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS` - max parallel runs, default `2`
- `BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE` - max retained profiles per scope before bridge rejects creation of a new one, default `3`
- `BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS` - idle/stale profile retention TTL in seconds, default `604800` (7 days), `0` disables TTL pruning
- `BROWSER_USE_BRIDGE_BROWSER_READY_RETRIES` - retry count for early transient browser readiness failures such as `CDP client not initialized`, default `2`
- `BROWSER_USE_BRIDGE_BROWSER_READY_RETRY_DELAY_MS` - delay between readiness retries in milliseconds, default `750`
- `BROWSER_USE_BRIDGE_LLM_PROVIDER` - legacy fallback: `browser_use`, `google`, or `anthropic`
- `BROWSER_USE_BRIDGE_LLM_MODEL` - legacy fallback model override for selected provider

Bridge автоматически устанавливает `BROWSER_USE_HOME` в `BROWSER_USE_BRIDGE_DATA_DIR`, если он не задан явно.

Для route inheritance Oxide Agent теперь может передавать provider secret server-to-server через header `X-Oxide-Browser-Llm-Api-Key`, не сохраняя его в request body и без обязательного env passthrough в sidecar.

## Request-Level `browser_llm_config`

Bridge принимает нормализованный `browser_llm_config` в `POST /sessions/run`.

Пример для `ZAI`:

```json
{
  "task": "Open the docs site and summarize the landing page",
  "browser_llm_config": {
    "provider": "zai",
    "model": "glm-5-turbo",
    "api_base": "https://api.z.ai/api/coding/paas/v4/chat/completions",
    "api_key_ref": "env:ZAI_API_KEY",
    "supports_vision": false
  }
}
```

Пример для `MiniMax`:

```json
{
  "task": "Open the pricing page and capture the main tiers",
  "browser_llm_config": {
    "provider": "minimax",
    "model": "MiniMax-M2.7",
    "api_key_ref": "env:MINIMAX_API_KEY",
    "supports_vision": true
  }
}
```

Поддержанные request-level provider-ы:

- `browser_use`
- `google`
- `anthropic`
- `minimax`
- `zai`
- `openrouter`
- `openai_compatible`

Секреты могут приходить двумя способами:

- server-to-server header `X-Oxide-Browser-Llm-Api-Key` для inherited route из Oxide Agent
- `api_key_ref` формата `env:KEY` для ручного request-level режима

## Run Locally

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -r services/browser_use_bridge/requirements.txt
uvicorn services.browser_use_bridge.app.main:app --host 0.0.0.0 --port 8000
```

## Run In Docker Compose

- Stage 2 wiring publishes the service on `127.0.0.1:8002` and keeps browser state in the `browser-use-data` volume.
- Default compose now assumes route inheritance as the primary path and no longer passes `BROWSER_USE_BRIDGE_LLM_PROVIDER` / `BROWSER_USE_BRIDGE_LLM_MODEL` into the sidecar.
- If you need the legacy env fallback, inject `BROWSER_USE_BRIDGE_LLM_PROVIDER`, `BROWSER_USE_BRIDGE_LLM_MODEL`, and the matching API key through a compose override or direct container environment.
- Stage C Rust provider automatically injects `browser_llm_config` from the active Oxide route for `gemini`, `minimax`, `zai`, and `openrouter`.
- Stage D secret handling sends inherited-route API keys via `X-Oxide-Browser-Llm-Api-Key`, so `minimax`, `zai`, and `openrouter` do not require dedicated sidecar env passthrough in the default compose setup.
- Stage 2 profile reuse injects `profile_scope` from runtime context; bridge enforces scope match on `profile_id` reuse and rejects creating more than `BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE` retained profiles in one scope.
- Stage 3 lifecycle cleanup detaches reusable profiles on graceful shutdown, auto-recovers orphaned `active` profiles left after bridge restarts/crashes, and prunes expired idle/stale profiles by TTL before quota checks.
- Stage 4 browser readiness hardening retries a narrow set of transient startup/runtime errors by recreating the browser before failing the session.
- The next warmup slice adds a short preflight wait before `Agent.run()`, so freshly created browser runtimes get a chance to connect before the first navigation step starts.
- The next post-run slice classifies returned `browser_use` history objects, so a run no longer counts as success merely because `Agent.run()` returned without a Python exception.
- The next navigation-only slice applies a stricter `Agent` preset for Rust steering tasks, so screenshot/extract-oriented runs get `enable_planning=False`, `use_judge=False`, `max_actions_per_step=1`, and an extra system-level navigation-only contract inside the bridge.
- The next execution-mode slice makes that split explicit: Rust provider now sends `execution_mode=autonomous|navigation_only`, and bridge persists the resolved mode into session metadata.
- The next keep-alive slice requests upstream `keep_alive=True` for `navigation_only` runs, so follow-up screenshot/extract tools can reuse the same live browser runtime after `Agent.run()` returns.
- Stage 5 verification adds focused test coverage for readiness retry budget exhaustion and health/env observability for retry knobs.
- P1 housekeeping now reconciles orphaned profiles against live session snapshots, so unrelated profile create/reuse/close operations do not accidentally mark an actually attached profile as `stale`.
- If you use request-level `browser_llm_config` with `api_key_ref=env:...`, the referenced env var must exist inside the `browser_use` container.
- Reusable profile metadata lives under `BROWSER_USE_BRIDGE_DATA_DIR/profiles/<profile_id>/metadata.json`, browser state under `.../profiles/<profile_id>/browser/`.
- Compose readiness uses `GET /health`, which returns HTTP `503` if the `browser_use` runtime failed to import.
- `GET /health` also shows whether legacy env fallback is configured, which LLM source is preferred, the profile idle TTL, readiness retry settings, whether session-level runtime observability is available, and whether orphan recovery is enabled.

## Notes

- `POST /sessions/run` создает новую сессию, если `session_id` не передан.
- При передаче существующего `session_id` bridge пытается reuse уже открытый browser runtime.
- Если `reuse_profile=true`, bridge создает отдельный reusable profile и возвращает `profile_id`, `profile_scope`, `profile_status`, `profile_attached`, `profile_reused`.
- Если передан `profile_id`, bridge пытается поднять новую browser session поверх сохраненного profile state и проверяет совпадение injected `profile_scope`.
- Если bridge был перезапущен с незакрытой profiled session, следующий reuse автоматически переведет orphaned profile из `active` в recoverable state и переиспользует его без ручной правки metadata.
- Если `browser_use` падает на раннем transient browser error вроде `CDP client not initialized`, bridge пытается пересоздать browser и повторить run вместо немедленного `failed`.
- Даже до старта первого agent step bridge теперь делает короткий readiness preflight, чтобы initial navigation реже упиралась в freshly-started CDP race.
- Если `browser_use` вернул internal failed history без Python exception, bridge теперь помечает run как `failed`; readiness-like history errors все еще могут получить bridge-side retry.
- Если Rust provider уже переписал task в navigation-only steering form, bridge теперь не ограничивается prompt rewrite и дополнительно сужает upstream `Agent` preset, чтобы тот реже уходил в screenshot/PDF/extract overreach.
- `POST /sessions/run`, `GET /sessions/{id}`, и `DELETE /sessions/{id}` теперь также возвращают `execution_mode`, чтобы было видно, шла ли задача как full autonomous run или как strict navigation-only run.
- Для `navigation_only` bridge теперь дополнительно просит upstream browser runtime остаться живым после `Agent.run()`, поэтому follow-up `extract_content` / `screenshot` должны чаще работать без немедленного rerun.
- Если follow-up tool вызывается после того, как upstream runtime уже умер или reset-нулся, bridge возвращает terminal `browser_session_not_alive` и очищает stale browser handle из session metadata in-memory state.
- При `close_session`, shutdown bridge и retry-reset такой kept-alive runtime теперь убивается принудительно, чтобы не оставлять фоновые browser processes.
- `POST /sessions/{id}/extract_content` читает текущую страницу активной сессии и возвращает `text` или `html` с optional truncation.
- `POST /sessions/{id}/screenshot` сохраняет PNG artifact в `BROWSER_USE_BRIDGE_DATA_DIR/artifacts/<session_id>/` и возвращает metadata с путем к файлу.
- Метаданные сессий сохраняются в `BROWSER_USE_BRIDGE_DATA_DIR/sessions/`.
- Idle/stale profile records старше `BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS` удаляются автоматически вместе с browser state, чтобы не зависал per-scope quota.
- `POST /sessions/run` и `GET /sessions/{id}` теперь возвращают `llm_source`, `llm_provider`, `llm_transport`, `vision_mode`, profile metadata, а также `browser_runtime_alive`, `browser_runtime_last_check_at`, `browser_runtime_dead_reason`, чтобы было видно, жив ли runtime сессии и почему bridge считает его закрытым.
- Реальная успешность `run_task` зависит от доступности `browser-use`, выбранного adapter-а и корректного secret resolution.
