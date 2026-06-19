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

Durable storage is SQLx/Postgres only. Old object-storage data is intentionally not imported, read, or dual-written.

## 2. Start a stack

Telegram full profile:

```bash
docker compose -f docker-compose.yml up --build -d
```

Telegram embedded profile:

```bash
docker compose -f docker-compose.telegram.yml up --build -d
```

Web console with remote Postgres from `.env`:

```bash
docker compose -f docker-compose.web.yml up --build -d
```

Web console with local Postgres and full-stack CRW (search + JS rendering):

```bash
docker compose -f docker-compose.web.yml -f docker-compose.web.local-services.yml up --build -d
```

## 3. Postgres and migrations

- Use PostgreSQL 15+ or Supabase Postgres.
- Keep `OXIDE_DATABASE_MAX_CONNECTIONS=5` unless the database pool limit is known.
- Docker images include migrations at `/app/migrations`.
- `docker-compose.web.yml` enables startup migrations by default to avoid first-boot races on fresh remote databases.
- For production/Supabase, `OXIDE_DATABASE_MIGRATE_ON_STARTUP=false` is safe only when a separate migration step is guaranteed before app startup.
- `docker-compose.web.local-services.yml` provides local Postgres on `127.0.0.1:55432`; base `docker-compose.web.yml` expects a remote `OXIDE_DATABASE_URL`.
- Keep `OXIDE_WEB_TASK_FILE_MAX_BYTES=33554432` unless WAL, backups, and retention are reviewed.

Retention cleanup helpers are bounded and opt-in; no scheduled deletion policy is enabled by default.

## 4. Optional services

Local sidecars (full-stack CRW + local Postgres for web):

```bash
docker compose -f docker-compose.telegram.yml -f docker-compose.telegram.local-services.yml up --build -d
docker compose -f docker-compose.web.yml -f docker-compose.web.local-services.yml up --build -d
```

The local-services overlays provide full-stack CRW (external project, AGPL-3.0, https://github.com/us/crw):
- **crw** — web crawler/scrape server (`ghcr.io/us/crw`)
- **searxng** — meta-search engine for `/v1/search` (`searxng/searxng`, needs `docker/searxng/settings.yml`)
- **lightpanda** — lightweight headless browser for JS rendering (`/v1/scrape` with `renderJs:true`)

The base compose files run CRW in single-container mode (scrape/markdown only, no search or JS rendering). The local-services overlays upgrade to full-stack by adding SearXNG and LightPanda sidecars. All three containers communicate on the default bridge network by service name; the app (host-network mode) reaches CRW via the published `127.0.0.1:3000` port.

SearXNG requires a custom `settings.yml` (`docker/searxng/settings.yml`) that enables JSON format and disables the built-in limiter. Without it, SearXNG returns 403 on JSON API requests. Generate a secret key with `openssl rand -hex 32` and set `SEARXNG_SECRET_KEY` in `.env`.

The CRW image (debian:bookworm-slim) contains no wget or curl, so healthchecks use bash `/dev/tcp` probes. Loopback self-hosted CRW needs no API key — `OXIDE_CRW_API_TOKEN` can stay empty.

External CRW, Kokoro, and Silero are configured through `.env.example`. If a service URL is unset, the related tool is disabled or falls back to its compiled default. The web compose entrypoint defaults `OXIDE_WEB_CRAWLER_MERGE=true`, so web tasks see one `web_crawler` URL-to-Markdown tool backed by webfetch first and CRW scrape fallback; set it to `false` to expose split lightweight `web_markdown` fetches.

Browser Live sidecar: the `browser-sidecar` service is included in the web and telegram Compose files but is disabled by default. To enable it, set `BROWSER_AGENT_ENABLED=true` and a non-empty `BROWSER_AGENT_SIDECAR_TOKEN` in `.env`, then verify health at `http://127.0.0.1:8787/healthz`. See `docs/browser-live.md` for the full setup checklist and troubleshooting.

## 5. Sandbox

Docker Compose uses the broker backend. Only `sandboxd` mounts `/var/run/docker.sock`; bot/web containers talk to it over `SANDBOXD_SOCKET=/run/sandboxd/sandboxd.sock`.

## 6. Verify

Web health:

```bash
curl -fsS http://127.0.0.1:3010/health
docker compose -f docker-compose.web.yml logs -f oxide_web sandboxd
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
docker compose -f docker-compose.web.yml down
```
