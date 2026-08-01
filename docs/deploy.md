# Deploy

Supported deployment entrypoints only. Use `.env.example` for the full variable reference.

## 1. Prepare env

```bash
git clone https://github.com/0FL01/oxide-agent.git
cd oxide-agent
cp .env.example .env
$EDITOR .env
```

Required for first boot:

| Area | Variables |
| --- | --- |
| Storage | `OXIDE_DATABASE_URL` or `DATABASE_URL` |
| LLM | Provider key plus `AGENT_MODEL_*` and `SUB_AGENT_MODEL_*` |
| Telegram | `TELEGRAM_TOKEN`, `TELEGRAM_ALLOWED_USERS` |
| Web | `OXIDE_WEB_BOOTSTRAP_TOKEN` when registration/bootstrap is enabled |

Permanent Life Mode bridge rollout uses explicit solo-owner bindings rather than token linking: set `LIFE_OWNER_WEB_LOGIN` for the Web `/life` owner, and set `LIFE_TELEGRAM_BOT_TOKEN` plus `LIFE_TELEGRAM_CHAT_ID` for the dedicated Life Telegram bot/chat. Keep `LIFE_TELEGRAM_BOT_TOKEN` separate from the ordinary Agent Mode `TELEGRAM_TOKEN` unless you intentionally run the same bot process for both roles.

Durable storage is SQLx/Postgres only. Old object-storage data is intentionally not imported, read, or dual-written.

## 2. Start the stack

Unified stack (Telegram + Web + Life Mode bridge):

```bash
docker compose -f docker-compose.yml up --build -d
```

Runs `oxide_agent` (Telegram bot), `oxide_web` (web console + Life executor + delivery worker), `sandboxd`, and `browser-sidecar` in one stack. CRW is not included — point `OXIDE_CRW_BASE_URL` at a remote CRW instance in `.env`.

## 3. Postgres and migrations

- Use PostgreSQL 15+ or Supabase Postgres.
- Keep `OXIDE_DATABASE_MAX_CONNECTIONS=5` unless the database pool limit is known.
- Docker images include migrations at `/app/migrations`.
- `docker-compose.yml` enables startup migrations on the Web service by default to avoid serving traffic on a stale schema.
- For production/Supabase, `OXIDE_DATABASE_MIGRATE_ON_STARTUP=false` is safe only when a separate migration step is guaranteed before app startup.
- Root Compose expects a reachable Postgres through `OXIDE_DATABASE_URL`.
- Keep `OXIDE_WEB_TASK_FILE_MAX_BYTES=33554432` unless WAL, backups, and retention are reviewed.

Retention cleanup helpers are bounded and opt-in; no scheduled deletion policy is enabled by default.

## 4. Optional services

Root Compose does not include CRW. Point `OXIDE_CRW_BASE_URL` and `OXIDE_CRW_API_TOKEN` at a separately managed CRW instance; Oxide exposes CRW-backed tools only when both values are configured.

External CRW, Kokoro, and Silero are configured through `.env.example`. If a service URL is unset, the related tool is disabled or falls back to its compiled default. The Web service defaults `OXIDE_WEB_CRAWLER_MERGE=true`, so web tasks see one `web_crawler` URL-to-Markdown tool backed by webfetch first; anti-bot, HTTP 403, and HTTP 429 failures fall back once to CRW Lightpanda. Explicit Lightpanda and Playwright modes run only the selected renderer. Set merge mode to `false` to expose split lightweight `web_markdown` fetches.

Browser Live sidecar: the `browser-sidecar` service is included in root Compose but is disabled by default. To enable it, set `BROWSER_AGENT_ENABLED=true` and a non-empty `BROWSER_AGENT_SIDECAR_TOKEN` in `.env`, then verify health at `http://127.0.0.1:8787/healthz`. See `docs/browser-live.md` for the full setup checklist and troubleshooting.

## 5. Sandbox

Docker Compose uses the sandboxd broker backend exclusively. Only `sandboxd` mounts `/var/run/docker.sock`; bot/web containers talk to it over `SANDBOXD_SOCKET=/run/sandboxd/sandboxd.sock`.

## 6. Verify

Web health:

```bash
curl -fsS http://127.0.0.1:3010/health
docker compose -f docker-compose.yml logs -f oxide_web sandboxd
```

Telegram logs and capabilities:

```bash
docker compose -f docker-compose.yml logs -f oxide_agent sandboxd
docker compose -f docker-compose.yml run --rm oxide_agent ./oxide-agent-telegram-bot capabilities --compiled --json
```

Check logs for SQL health, migration errors, and sandbox broker health before enabling traffic.

## 7. Operate

Update:

```bash
git pull
docker compose -f docker-compose.yml up --build -d
```

Stop:

```bash
docker compose -f docker-compose.yml down
```
