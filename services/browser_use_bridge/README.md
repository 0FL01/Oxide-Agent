# Browser Use Bridge

Минимальный HTTP bridge для интеграции `browser-use` в Oxide Agent.

## HTTP API

- `GET /health`
- `POST /sessions/run`
- `GET /sessions/{id}`
- `DELETE /sessions/{id}`

`POST /sessions/run` поддерживает два режима выбора LLM:

- request-level `browser_llm_config`, который уже используется Rust provider-ом для Stage C route inheritance и является основным режимом
- legacy fallback через `BROWSER_USE_BRIDGE_LLM_PROVIDER` / `BROWSER_USE_BRIDGE_LLM_MODEL`

## Environment

- `BROWSER_USE_BRIDGE_HOST` - bind host, default `0.0.0.0`
- `BROWSER_USE_BRIDGE_PORT` - bind port, default `8000`
- `BROWSER_USE_BRIDGE_DATA_DIR` - data dir, default `/tmp/browser-use-bridge`
- `BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS` - default run timeout, default `120`
- `BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS` - max allowed timeout override, default `300`
- `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS` - max parallel runs, default `2`
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
- If you use request-level `browser_llm_config` with `api_key_ref=env:...`, the referenced env var must exist inside the `browser_use` container.
- Compose readiness uses `GET /health`, which returns HTTP `503` if the `browser_use` runtime failed to import.
- `GET /health` also shows whether legacy env fallback is configured and which LLM source is preferred.

## Notes

- `POST /sessions/run` создает новую сессию, если `session_id` не передан.
- При передаче существующего `session_id` bridge пытается reuse уже открытый browser runtime.
- Метаданные сессий сохраняются в `BROWSER_USE_BRIDGE_DATA_DIR/sessions/`.
- `POST /sessions/run` и `GET /sessions/{id}` теперь возвращают `llm_source`, `llm_provider`, `llm_transport` и `vision_mode`, чтобы было видно, исполнялся ли запрос через inherited route или legacy fallback.
- Реальная успешность `run_task` зависит от доступности `browser-use`, выбранного adapter-а и корректного secret resolution.
