# Deploy

This guide covers the supported deployment entrypoints only. Use `.env.example` as the full configuration reference.

## Choose target

| Target | Entrypoint | Notes |
| --- | --- | --- |
| Telegram, full | `docker-compose.yml` | Full profile, broker sandbox, SearXNG sidecar. |
| Telegram, embedded | `docker-compose.telegram.yml` | Smaller Telegram profile with broker sandbox. |
| Web console | `docker-compose.web.yml` | Web UI, broker sandbox, external search/crawl friendly. |
| Local search/crawl sidecars | `docker-compose.*.local-services.yml` | Optional SearXNG + Crawl4AI sidecars. |
| Bare host | release binary | Use `SANDBOX_BACKEND=bwrap`; see `docs/bwrap-sandbox.md`. |

## Prerequisites

- Docker Engine + Compose for Docker deployments.
- PostgreSQL 15+ or Supabase Postgres for durable production state.
- One transport credential: `TELEGRAM_TOKEN` or web bootstrap/login config.
- At least one LLM route, for example `OPENCODE_GO_API_KEY` + `AGENT_MODEL_*`.

## Docker deploy

```bash
git clone https://github.com/0FL01/oxide-agent.git
cd oxide-agent
cp .env.example .env
$EDITOR .env
```

Telegram full profile:

```bash
docker compose -f docker-compose.yml up --build -d
```

Telegram embedded profile:

```bash
docker compose -f docker-compose.telegram.yml up --build -d
```

Web console:

```bash
docker compose -f docker-compose.web.yml up --build -d
```

Telegram embedded with local SearXNG + Crawl4AI:

```bash
docker compose -f docker-compose.telegram.yml -f docker-compose.telegram.local-services.yml up --build -d
```

Web console with local SearXNG + Crawl4AI:

```bash
docker compose -f docker-compose.web.yml -f docker-compose.web.local-services.yml up --build -d
```

## Required env for first boot

| Area | Variables |
| --- | --- |
| Telegram | `TELEGRAM_TOKEN`, `TELEGRAM_ALLOWED_USERS` |
| Web | `OXIDE_WEB_BOOTSTRAP_TOKEN` for first admin registration when enabled |
| Storage | `OXIDE_DATABASE_URL` or `DATABASE_URL`; optional `OXIDE_DATABASE_MAX_CONNECTIONS`, `OXIDE_DATABASE_CONNECT_TIMEOUT_SECS`, `OXIDE_DATABASE_MIGRATE_ON_STARTUP`, `OXIDE_DATABASE_MIGRATIONS_DIR` |
| LLM | Provider key, `AGENT_MODEL_ID`, `AGENT_MODEL_PROVIDER`, `SUB_AGENT_MODEL_ID`, `SUB_AGENT_MODEL_PROVIDER` |

The complete variable list lives in `.env.example`.

SQLx/Postgres is the durable storage backend. Previous object-storage data is intentionally not imported, read, or dual-written. Keep `OXIDE_DATABASE_MIGRATE_ON_STARTUP=false` for production/Supabase unless deployment explicitly runs migrations at startup.

## Optional external services

External services are optional. Use them when SearXNG, Crawl4AI, TTS, or similar services already run on WAN behind HTTPS, reverse proxy, or Bearer auth. If a service is not configured, the related tool is disabled, unavailable, or falls back to its compiled default depending on the module.

For local Docker sidecars instead of external WAN services, add the matching overlay:

```bash
docker compose -f docker-compose.telegram.yml -f docker-compose.telegram.local-services.yml up --build -d
docker compose -f docker-compose.web.yml -f docker-compose.web.local-services.yml up --build -d
```

The Telegram local-services overlay also enables the Crawl4AI tool at build time.

SearXNG external HTTPS instance:

```env
SEARXNG_ENABLED=true
SEARXNG_URL=https://searxng.example.com
SEARXNG_BEARER_TOKEN=optional-token
```

Crawl4AI external HTTPS instance:

```env
OXIDE_CRAWL4AI_BASE_URL=https://crawl4ai.example.com
OXIDE_CRAWL4AI_API_TOKEN=optional-token
```

Kokoro TTS external service:

```env
KOKORO_TTS_URL=https://kokoro.example.com
KOKORO_TTS_VOICE=af_heart
KOKORO_TTS_FORMAT=ogg
KOKORO_TTS_TIMEOUT_SECS=60
```

Silero TTS external service:

```env
SILERO_TTS_URL=https://silero.example.com
SILERO_TTS_SPEAKER=baya
SILERO_TTS_FORMAT=ogg
SILERO_TTS_SAMPLE_RATE=48000
SILERO_TTS_TIMEOUT_SECS=60
```

Bearer token variables are optional. If the token variable is empty or unset, Oxide sends requests without `Authorization`.

Do not deploy these services from this guide. Deploy them separately, then point Oxide to their HTTPS base URLs.

## Sandbox

Docker Compose uses the broker backend by default:

```env
SANDBOX_BACKEND=broker
SANDBOXD_SOCKET=/run/sandboxd/sandboxd.sock
```

Only `sandboxd` mounts `/var/run/docker.sock`; the main bot/web container talks to `sandboxd` over a Unix socket.

Bare-host Bubblewrap mode uses:

```env
SANDBOX_BACKEND=bwrap
```

For rootfs and host setup, see `docs/bwrap-sandbox.md`.

## Operations

Logs:

```bash
docker compose -f docker-compose.yml logs -f oxide_agent sandboxd
docker compose -f docker-compose.web.yml logs -f oxide_web sandboxd
```

Update:

```bash
git pull
docker compose -f docker-compose.yml up --build -d
```

Stop:

```bash
docker compose -f docker-compose.yml down
```

Verify compiled capabilities:

```bash
docker compose -f docker-compose.yml run --rm oxide_agent ./oxide-agent-telegram-bot capabilities --compiled --json
```
