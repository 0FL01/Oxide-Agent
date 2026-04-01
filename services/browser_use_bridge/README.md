# Browser Use Bridge

Минимальный HTTP bridge для интеграции `browser-use` в Oxide Agent.

## HTTP API

- `GET /health`
- `POST /sessions/run`
- `GET /sessions/{id}`
- `DELETE /sessions/{id}`

## Environment

- `BROWSER_USE_BRIDGE_HOST` - bind host, default `0.0.0.0`
- `BROWSER_USE_BRIDGE_PORT` - bind port, default `8000`
- `BROWSER_USE_BRIDGE_DATA_DIR` - data dir, default `/tmp/browser-use-bridge`
- `BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS` - default run timeout, default `120`
- `BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS` - max allowed timeout override, default `300`
- `BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS` - max parallel runs, default `2`
- `BROWSER_USE_BRIDGE_LLM_PROVIDER` - `browser_use`, `google`, or `anthropic`
- `BROWSER_USE_BRIDGE_LLM_MODEL` - optional model override for selected provider

Bridge автоматически устанавливает `BROWSER_USE_HOME` в `BROWSER_USE_BRIDGE_DATA_DIR`, если он не задан явно.

## Run Locally

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -r services/browser_use_bridge/requirements.txt
uvicorn services.browser_use_bridge.app.main:app --host 0.0.0.0 --port 8000
```

## Notes

- `POST /sessions/run` создает новую сессию, если `session_id` не передан.
- При передаче существующего `session_id` bridge пытается reuse уже открытый browser runtime.
- Метаданные сессий сохраняются в `BROWSER_USE_BRIDGE_DATA_DIR/sessions/`.
- Реальная успешность `run_task` зависит от доступности `browser-use` и выбранного LLM provider.
