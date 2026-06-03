**Executive summary:** Выбранный дизайн: Rust-native `ToolRuntime` provider `crawl4ai_markdown`, который ходит в уже поднятый self-hosted Crawl4AI REST API через `POST /crawl`. Это самый простой вариант: Oxide остаётся Rust-only агентом, Crawl4AI остаётся внешним browser-rendering сервисом, tool schema маленькая и статичная, а недоступность сервиса возвращается как structured runtime failure, не как изменение списка tools.

Ни Python subprocess, ни MCP, ни Docker lifecycle внутри Oxide здесь не нужны. Единственная неизбежная cache-hit цена — если этот tool намеренно включить в активный профиль, provider tool-surface изменится один раз; дальше он должен быть полностью стабильным.

---

## План по чанкам

### Чанк 0 — baseline RECON

Что coder должен сначала зафиксировать в текущем Oxide Agent:

1. Проверить сборку текущего проекта до изменений.

   Baseline проверка до изменений выполнена: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.

2. Подтвердить reference provider:

   `crates/oxide-agent-core/src/agent/providers/webfetch_md.rs`

   Это правильный ближайший образец: native provider, `ToolExecutor`, `ToolDefinition`, `reqwest`, timeout, cancellation, URL validation, bounded body, structured failure.

3. Зафиксировать cache-sensitive пути:

   `crates/oxide-agent-core/src/agent/executor/registry.rs`

   `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`

   `crates/oxide-agent-core/src/agent/prompt/composer.rs`

   `crates/oxide-agent-core/src/capabilities/compiled.rs`

   `crates/oxide-agent-core/Cargo.toml`

4. Учесть два дополнительных места, которые не были в начальном списке, но реально участвуют в tool-surface:

   `crates/oxide-agent-core/src/agent/providers/delegation.rs`

   `crates/oxide-agent-core/src/agent/providers/manager_control_plane/agent_controls.rs`

   Также рядом есть:

   `crates/oxide-agent-core/src/agent/providers/manager_control_plane/mod.rs`

5. Проверить текущую registry модель.

   В текущем архиве registry строится детерминированно, tool names идут через `BTreeMap`. Поэтому новый tool не надо вручную “ставить в порядок”. Надо только зарегистрировать его тем же способом, что `WebFetchMdToolModule`.

6. Проверить текущие зависимости.

   `reqwest` уже есть как optional dependency. Для этого tool не нужен `htmd`, потому что markdown делает Crawl4AI. Для jitter уже есть `fastrand`. Для DNS preflight можно использовать `tokio::net::lookup_host`. Новых crates для v1 добавлять не надо.

7. Не чинить unrelated issues.

   Если coder заметит, что `webfetch_md.rs` не проверяет DNS-resolved private IP для domain hosts или IPv6-mapped IPv4, это стоит отметить, но не превращать текущую задачу в hardening всего web stack. Для нового `crawl4ai_markdown` эти проверки обязательны.

---

### Чанк 1 — integration decision

Сравнение вариантов:

1. **Rust provider + Crawl4AI REST API — выбрать.**

   Это KISS-дизайн. Oxide добавляет маленький native provider, ходит через `reqwest`, не тащит Python, Playwright, Chromium, browser pool, Docker API или MCP. Crawl4AI self-hosted server уже предоставляет REST API на стандартном порту `11235`; официальные self-hosting примеры показывают `POST /crawl` с `BrowserConfig` и `CrawlerRunConfig`. ([Crawl4AI Documentation][1])

2. **Rust provider + Python subprocess runner — не выбирать.**

   Это добавит Python runtime coupling, lifecycle subprocess, stdout/stderr parsing, dependency drift и failure modes внутри Oxide. При наличии self-hosted REST API это лишний слой.

3. **MCP integration — не выбирать.**

   Crawl4AI server действительно exposes MCP endpoints и MCP tools, включая markdown, screenshot, pdf, execute_js и crawl, но Oxide уже имеет native ToolRuntime architecture. MCP здесь только расширит surface area и добавит ещё один protocol boundary без пользы для single-URL markdown tool. ([Crawl4AI Documentation][1])

4. **Direct Rust/browser implementation — не выбирать.**

   Это фактически создание второго Crawl4AI: browser runtime, JS rendering, overlays, anti-bot behavior, cache, process isolation. Для личного/малого агента это overengineering.

5. **Docker lifecycle management inside Oxide — не выбирать.**

   Oxide не должен стартовать/стопать Crawl4AI container. Deployment остаётся отдельной concern. Rust provider должен только обращаться к configured base URL и возвращать structured failure, если сервис недоступен.

Итог: **вариант 1, REST adapter через `POST /crawl`.**

---

### Чанк 2 — tool contract

Tool name строго:

`crawl4ai_markdown`

Tool description должен быть коротким и стабильным:

`Open one http/https URL with the configured Crawl4AI REST service and return bounded Markdown. Use for pages that need browser rendering, JavaScript, overlay/consent handling, or when web_markdown fails. This tool does not crawl multiple pages, execute JavaScript, run hooks, use LLM extraction, or return screenshots/PDFs.`

Final schema:

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "Fully-qualified public http/https URL to open"
    },
    "timeout_secs": {
      "type": "integer",
      "description": "Optional request timeout in seconds, clamped to configured bounds"
    },
    "wait_for": {
      "type": "string",
      "description": "Optional CSS selector to wait for before extracting Markdown; JavaScript conditions are not allowed"
    },
    "fresh": {
      "type": "boolean",
      "description": "If true, bypass Crawl4AI content cache for this crawl; default false"
    },
    "max_chars": {
      "type": "integer",
      "description": "Optional maximum Markdown characters to return, clamped to configured hard cap"
    }
  },
  "required": ["url"],
  "additionalProperties": false
}
```

Argument semantics:

`url` is required. Accept only `http` and `https`. Reject `file`, `raw`, `data`, `ftp`, `chrome`, `about`, local paths, and anything without a host.

`timeout_secs` default: `60`. Minimum: `1`. Maximum: `OXIDE_CRAWL4AI_MAX_TIMEOUT_SECS`, default `120`.

`wait_for` is optional. Accept only CSS. Allow either `.main`, `#article`, `main article`, or `css:.main`. Normalize to Crawl4AI’s CSS wait format. Reject `js:`, `function`, `=>`, braces, semicolons, newlines, and overly long strings. Keep max length around `256` chars. Crawl4AI supports both CSS and JS wait conditions, but this tool must only expose CSS because arbitrary JS would turn it into browser automation. ([Crawl4AI Documentation][2])

`fresh` default: `false`. If `false`, use Crawl4AI cache mode `enabled`. If `true`, use `bypass`. Crawl4AI cache modes are content-fetch cache controls, not LLM provider prompt cache controls. ([Crawl4AI Documentation][3])

`max_chars` default: `20_000`. Hard cap: `OXIDE_CRAWL4AI_MAX_OUTPUT_CHARS`, default `30_000`.

Do not add `headers`, `cookies`, `proxy`, `user_agent`, `browser profile`, `endpoint selector`, `auth token`, `base URL override`, `session_id`, `js`, `hooks`, `screenshot`, `pdf`, `deep crawl`, or arbitrary Crawl4AI config passthrough.

---

### Чанк 3 — Rust provider

Создать новый файл:

`crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown.rs`

Статус: первичный provider-contract slice реализован. Добавлены atomic feature gate, cfg-gated provider module/export, provider file, static tool schema, env config parsing, URL/SSRF/DNS preflight, health check, bounded REST response/output, structured failure payloads, and provider-local unit tests.

Provider shape:

1. `Crawl4AiMarkdownProvider`.

2. `Crawl4AiMarkdownToolExecutor`.

3. `tool_runtime_executors()` mirrors `WebFetchMdProvider`.

4. `ToolDefinition` is static, built from constants, never from env, health, base URL, version, or runtime state.

5. Parse args with `serde`.

6. Validate URL before any call to Crawl4AI.

7. SSRF/private IP rejection before REST call:

   Reject localhost domains, `.localhost`, loopback, private IPv4, link-local, broadcast, documentation, unspecified, `169.254.169.254`, IPv6 loopback, IPv6 unspecified, IPv6 unique-local, IPv6 link-local, IPv4-mapped IPv6 private/metadata/local addresses.

   For domain hosts, do a DNS preflight using `tokio::net::lookup_host(host, port)` and reject if any resolved IP is unsafe. This is stricter than current `webfetch_md` and is required here because Crawl4AI will be the component doing the actual fetch.

8. Acknowledge the DNS rebinding limitation.

   Rust preflight cannot perfectly prevent DNS rebinding because Crawl4AI resolves again inside its container. Simple mitigation: do Rust preflight anyway, validate final URL after response, and harden the Crawl4AI container/network so it cannot reach metadata/private networks in higher-risk deployments. Do not build a proxy gateway inside Oxide.

9. Build `reqwest::Client`.

   Use existing `reqwest`. Add default client timeout at or above max tool timeout plus health overhead. Do not add new HTTP crates.

10. Health check.

Use `GET /health` with `OXIDE_CRAWL4AI_HEALTH_TIMEOUT_MS`, default `1500`. Crawl4AI self-hosting docs show `/health` returning a small JSON health object. ([Crawl4AI Documentation][1])

If health fails, return a normal tool failure with:

`provider_unavailable: true`

`retryable: true`

`error_kind: "crawl4ai_unavailable"`

Do not unregister the tool.

11. POST `/crawl`.

Non-streaming, one URL, no jobs, no webhooks.

12. Cancellation.

Respect `invocation.cancellation_token` before health, before jitter, before POST, and while reading response body.

13. Bounded Crawl4AI response body.

Hard-code a response JSON body limit for v1, for example `10 MiB`. Do not make this another config knob unless real usage proves it necessary. Crawl4AI `CrawlResult` can include HTML, cleaned HTML, links, media, screenshots/PDFs when enabled; the provider must fail safely if the service returns too much. ([Crawl4AI Documentation][4])

14. Output normalization.

Return only bounded markdown and bounded metadata. Do not return HTML, cleaned HTML, links, media, headers, screenshot, PDF, network logs, or console messages.

15. Structured errors.

Match the style of `webfetch_md`: failed tool call should still produce `ToolOutput` failure with `structured_payload`, not crash the executor.

---

### Чанк 4 — Crawl4AI REST payload/response mapping

Endpoint choice:

`POST /crawl`

Reason:

`/crawl` lets Rust send explicit `browser_config` and `crawler_config` while still staying single URL and markdown-only. Current self-hosting docs show `browser_config` as `{ "type": "BrowserConfig", "params": { ... } }` and `crawler_config` as `{ "type": "CrawlerRunConfig", "params": { ... } }` in `/crawl` examples. ([Crawl4AI Documentation][1])

Do not choose `/md` for v1. Current self-hosting docs show `/md` heavily in LLM-style examples with `f: "llm"`, providers, temperature, and custom LLM base URLs; that is the wrong contract for this tool. ([Crawl4AI Documentation][1])

Minimal request intent:

```json
{
  "urls": ["<validated_url>"],
  "browser_config": {
    "type": "BrowserConfig",
    "params": {
      "browser_type": "chromium",
      "headless": true,
      "java_script_enabled": true,
      "enable_stealth": true
    }
  },
  "crawler_config": {
    "type": "CrawlerRunConfig",
    "params": {
      "stream": false,
      "cache_mode": "enabled|bypass",
      "page_timeout": 60000,
      "wait_until": "domcontentloaded",
      "remove_overlay_elements": true,
      "remove_consent_popups": true,
      "simulate_user": true,
      "override_navigator": true
    }
  }
}
```

This is not implementation code; it is the contract the Rust structs should serialize.

Notes:

1. `urls` must contain exactly one URL.

2. `stream` must be false.

3. `cache_mode`:

   `fresh=false` → `enabled`

   `fresh=true` → `bypass`

4. `page_timeout` comes from bounded `timeout_secs` converted to milliseconds.

5. `wait_for` is included only if user gave a safe CSS selector.

6. Do not include `js_code`, `js_code_before_wait`, `c4a_script`, hooks, screenshots, PDF, MHTML, download options, proxy config, session id, extraction strategy, LLM config, webhook config, deep crawl strategy, or multiple URLs.

7. `enable_stealth`, `remove_overlay_elements`, `remove_consent_popups`, `simulate_user`, and `override_navigator` are allowed fixed options because Crawl4AI documents them and they directly serve the “JS/overlay/anti-bot-lite” use case without exposing automation to the LLM. `magic` is documented as experimental; leave it out in v1 unless a real smoke test shows it is necessary. ([Crawl4AI Documentation][2])

Response parsing:

1. Accept a Crawl4AI wrapper object with `results`.

2. Accept exactly one result.

3. If result count is not one, return `error_kind: "unexpected_result_count"`.

4. If `success=false`, return `error_kind: "crawl_failed"` and include bounded `error_message`.

5. Extract final URL from `result.url` or `redirected_url`, depending on actual response shape. Crawl4AI docs define `url` as final crawled URL after redirects and include `status_code` / `redirected_status_code` fields. ([Crawl4AI Documentation][4])

6. Validate final URL with the same URL/SSRF rules before returning markdown. If final URL is unsafe, discard markdown and return `error_kind: "final_url_blocked"`.

7. Markdown extraction should be robust:

   If `markdown` is a string, use it as `markdown_kind: "raw_markdown"`.

   If `markdown` is an object, prefer `raw_markdown`.

   Fallback order only if the preferred field is empty: `markdown_with_citations`, then `fit_markdown`.

   Set `markdown_kind` to the actual field used.

8. Trim leading/trailing whitespace.

9. Truncate to effective max chars.

10. Set `truncated`, `chars`, and optionally `elapsed_ms`.

---

### Чанк 5 — config and feature gating

Use these env/config names:

`OXIDE_CRAWL4AI_BASE_URL`

Default: `http://127.0.0.1:11235`

`OXIDE_CRAWL4AI_API_TOKEN`

Optional. If set, send `Authorization: Bearer <token>`. Crawl4AI self-hosting config supports JWT auth when security is enabled. ([Crawl4AI Documentation][1])

`OXIDE_CRAWL4AI_DEFAULT_TIMEOUT_SECS`

Default: `60`

`OXIDE_CRAWL4AI_MAX_TIMEOUT_SECS`

Default: `120`

`OXIDE_CRAWL4AI_MAX_OUTPUT_CHARS`

Default: `30_000`

`OXIDE_CRAWL4AI_HEALTH_TIMEOUT_MS`

Default: `1500`

`OXIDE_CRAWL4AI_JITTER_MIN_MS`

Default: `250`

`OXIDE_CRAWL4AI_JITTER_MAX_MS`

Default: `1500`

`OXIDE_CRAWL4AI_MAX_RETRIES`

Default: `0`

Do not add `OXIDE_CRAWL4AI_ENABLED` in v1 provider code. The current project model is Cargo/profile features plus module runtime config. Tool existence should be controlled by the `tool-crawl4ai-markdown` feature and existing module enable/disable mechanism, not by live env health. If the project owner later wants env-based module enablement, resolve it into static `AgentSettings.modules` before tool registry/prompt composition.

Config rules:

1. Env/config may change runtime behavior but must never change tool schema.

2. Base URL and token are operator config, not LLM args.

3. Secrets must never be logged.

4. Base URL host may be included in errors as `crawl4ai_base_url_host`; token and full auth-bearing URL must never be included.

5. Missing service returns structured runtime failure.

6. Dead service must not remove the tool from the registry.

---

### Чанк 6 — registry, capabilities, prompt

Статус: tool-surface wiring реализован. Добавлены `Crawl4AiMarkdownToolModule`, registry/capability manifest wiring, static prompt guidance, sub-agent delegation wiring, manager-control-plane search group, thought template, `.env.example` knobs, registry tests, and updated all-features modular registry snapshot. Feature не добавлен в existing profiles.

Files to change:

`crates/oxide-agent-core/Cargo.toml`

Add atomic feature:

`tool-crawl4ai-markdown = ["dep:reqwest"]`

Recommendation for cache stability: do not add this feature to every existing profile in the first PR. Add the atomic feature and let deployments opt in with `--features "profile-full tool-crawl4ai-markdown"` or a deliberate profile change. If project owner wants it in `profile-full`, do it as an intentional cache-surface rollout and update snapshots in the same PR.

`crates/oxide-agent-core/src/agent/providers/mod.rs`

Add cfg-gated module and export.

`crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`

Add `Crawl4AiMarkdownToolModule`, module id:

`tool/crawl4ai-markdown`

`crates/oxide-agent-core/src/agent/tool_runtime/mod.rs`

Re-export module behind feature.

`crates/oxide-agent-core/src/agent/executor/registry.rs`

Register the module behind `#[cfg(feature = "tool-crawl4ai-markdown")]`.

Do not make registration depend on health.

`crates/oxide-agent-core/src/capabilities/compiled.rs`

Add static capability manifest entry:

feature: `tool-crawl4ai-markdown`

module id: `tool/crawl4ai-markdown`

kind: `Search`

provides: `tool/crawl4ai-markdown`

`crates/oxide-agent-core/src/agent/prompt/composer.rs`

Add only one short static guidance line in the existing web research section, gated by tool presence:

`Use crawl4ai_markdown only when web_markdown fails or when a page needs browser rendering, JavaScript, or overlay/consent handling.`

Do not add current health, base URL, Crawl4AI version, Docker tag, request id, cache state, or dynamic readiness information to prompt.

`crates/oxide-agent-core/src/agent/providers/delegation.rs`

If sub-agents should have this tool when compiled, add `Crawl4AiMarkdownToolModule` exactly like `WebFetchMdToolModule`. If not, leave it out deliberately and document that crawl4ai is main-agent-only. My recommendation: include it for sub-agents only if `web_markdown` is already available to sub-agents in the same profile, because the use case is web research fallback.

`crates/oxide-agent-core/src/agent/providers/manager_control_plane/mod.rs`

Add topic-agent tool constant only if manager control plane exposes web tool groups.

`crates/oxide-agent-core/src/agent/providers/manager_control_plane/agent_controls.rs`

Add a static search group entry or extend the webfetch group. Keep aliases stable and short. Do not add health-dependent catalog entries.

Possible group:

provider: `crawl4ai`

aliases: `["search", "crawl4ai", "browser_markdown"]`

tools: `["crawl4ai_markdown"]`

Do not alias it as generic `web_markdown`; keep the name explicit.

Docs/env:

`.env.example`

Relevant README or local dev docs section.

No Python files. No Dockerfile changes required for Oxide image.

---

### Чанк 7 — tests

Статус: HTTP contract tests добавлены. Mock server покрывает `GET /health`, `POST /crawl` payload shape, auth header, success output extraction, health structured failure, and one retry for retryable crawl 5xx. Normal CI не требует real Crawl4AI/browser.

Required provider tests:

1. Tool definition snapshot/stability:

   Assert exact name, description, JSON schema, required fields, and `additionalProperties: false`.

2. Runtime executor lists exactly one tool:

   `crawl4ai_markdown`.

3. Registry stability:

   With feature enabled and dead base URL, registry still contains `crawl4ai_markdown`.

4. Disabled module behavior:

   If `tool/crawl4ai-markdown` is disabled through existing module runtime config, registry does not expose the tool.

5. Unsafe URL rejection:

   `file://`, `raw:`, `data:`, `ftp:`, no-host URL, localhost, `.localhost`.

6. Private IP rejection:

   `127.0.0.1`, `10.0.0.1`, `172.16.0.1`, `192.168.0.1`, `169.254.169.254`, link-local, broadcast, unspecified.

7. IPv6 rejection:

   `::1`, `::`, `fc00::/7`, `fe80::/10`, IPv4-mapped private and metadata addresses.

8. DNS preflight rejection:

   Use a local resolver strategy if easily testable. If not, isolate the pure IP classification tests and add one integration-ish test with `reqwest::Client::resolve` pattern where applicable.

9. Health unavailable:

   Mock `/health` connection refused or non-2xx returns structured failure with:

   `provider_unavailable: true`

   `retryable: true`

   `error_kind: "crawl4ai_unavailable"`

10. Auth:

If `OXIDE_CRAWL4AI_API_TOKEN` is set, mock server observes Authorization header. Logs/output must not contain the token.

11. REST non-2xx:

Bounded response tail, no panic, structured failure.

12. Crawl result `success=false`:

Structured `crawl_failed`.

13. Markdown string shape:

`markdown: "# Title"` parses correctly.

14. Markdown object shape:

`markdown.raw_markdown`, `markdown.markdown_with_citations`, `markdown.fit_markdown` fallback works.

15. Final URL blocked:

If Crawl4AI returns final URL as localhost/private/metadata, provider discards markdown and returns structured failure.

16. Timeout behavior.

17. Cancellation before health.

18. Cancellation during response body read.

19. Bounded response body.

20. Markdown truncation.

21. Retry behavior:

With default `OXIDE_CRAWL4AI_MAX_RETRIES=0`, no retry.

With max retries `1`, retry only retryable transport/5xx/429 cases.

22. Prompt snapshot:

If project has cache-sensitive prompt snapshots, update/add one that proves only the one static guidance line appears and no service state appears.

23. Registry/capability snapshots:

Update `modular_registry_snapshots` only for feature/profile combinations that intentionally include the new tool.

Test implementation should reuse the project’s existing style: `tokio::net::TcpListener` mock HTTP server, `insta` snapshots, no new mock server crate unless the test becomes unreadable.

Do not require real Crawl4AI/browser in normal CI. Add an ignored smoke test for local dev only.

---

### Чанк 8 — local/dev deployment notes

Add a short doc section, not a platform.

Recommended local command intent:

```bash
docker run -d \
  --name crawl4ai \
  -p 127.0.0.1:11235:11235 \
  --shm-size=1g \
  unclecode/crawl4ai:0.8.7
```

Docker Hub currently shows `unclecode/crawl4ai` tags including `0.8.7`, `0.8.6`, `0.8.5`, and `0.8.0`; GitHub README highlights v0.8.7 as a security-hardening release. Pin a specific tag, and for production prefer pinning by digest after `docker manifest inspect`. Do not use `latest` in production. ([Docker Hub][5])

The Crawl4AI self-hosting docs show port `11235` and a basic Docker run with `--shm-size=1g`; Docker Hub’s current example uses `--shm-size=3g`. Start with `1g` for local/dev, raise it only if browser crashes or large pages fail. ([Crawl4AI Documentation][1])

Service hardening notes:

1. Bind to loopback or private network.

2. Do not expose Crawl4AI API directly to the internet.

3. Enable JWT/API auth if reachable outside loopback/private trusted network.

4. Keep Crawl4AI rate limiting enabled; docs show `rate_limiting.enabled: True` with default `1000/minute`. ([Crawl4AI Documentation][1])

5. Do not mount host directories unless needed.

6. Do not grant extra container privileges.

7. Keep hooks disabled.

8. Disable or block `/execute_js`, `/screenshot`, `/pdf`, `/crawl/job`, `/llm/job`, and webhook endpoints at reverse proxy if the service is exposed beyond local trusted network.

9. Do not configure proxy rotation through Oxide. If proxy is needed, configure it on the Crawl4AI service side.

Security version requirement:

Do not run Crawl4AI versions below `0.8.0`. Public advisories describe a critical Docker API RCE in versions before `0.8.0` through `/crawl` hooks, and v0.8.0 release notes state hooks were disabled by default as a security fix. ([GitHub][6])

---

### Чанк 9 — delivery for coder

Coder should deliver one focused PR with:

1. New provider file.

2. Feature gate.

3. Registry/module/capability/prompt integration.

4. Tests.

5. Minimal docs/env example.

6. No Python.

7. No Docker lifecycle logic.

8. No MCP.

9. No arbitrary Crawl4AI config passthrough.

10. No prompt dynamic state.

Validation commands for coder:

```bash
cargo fmt --all --check
```

```bash
cargo check --workspace --no-default-features --features "tool-crawl4ai-markdown"
```

```bash
cargo test -p oxide-agent-core --no-default-features --features "tool-crawl4ai-markdown" crawl4ai_markdown
```

```bash
cargo test -p oxide-agent-core --no-default-features --features "tool-crawl4ai-markdown" typed_runtime_registry_exposes_crawl4ai_markdown_tool
```

```bash
cargo test -p oxide-agent-core --no-default-features --features "profile-full tool-crawl4ai-markdown"
```

```bash
cargo test -p oxide-agent-core --all-features modular_registry_snapshot
```

```bash
cargo clippy -p oxide-agent-core --no-default-features --features "tool-crawl4ai-markdown" --all-targets -- -D warnings
```

For local smoke test:

```bash
docker manifest inspect unclecode/crawl4ai:0.8.7
```

```bash
docker run -d --name crawl4ai -p 127.0.0.1:11235:11235 --shm-size=1g unclecode/crawl4ai:0.8.7
```

```bash
curl -fsS http://127.0.0.1:11235/health
```

Then run the ignored smoke test explicitly.

---

## Implementation contract для coder

### Architecture

Implement `crawl4ai_markdown` as a native Rust `ToolRuntime` provider.

Oxide side responsibilities:

1. Static tool schema.

2. URL validation.

3. DNS preflight and SSRF/private IP rejection.

4. Health check.

5. Bounded HTTP request to Crawl4AI REST.

6. Timeout/cancellation.

7. Bounded JSON response read.

8. Robust markdown extraction.

9. Final URL validation.

10. Output truncation.

11. Structured success/failure.

Crawl4AI side responsibilities:

1. Browser rendering.

2. JS page loading.

3. Overlay/consent removal.

4. Markdown generation.

5. Its own internal content cache.

### Exact files

Create:

`crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown.rs`

Modify:

`crates/oxide-agent-core/src/agent/providers/mod.rs`

`crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`

`crates/oxide-agent-core/src/agent/tool_runtime/mod.rs`

`crates/oxide-agent-core/src/agent/executor/registry.rs`

`crates/oxide-agent-core/src/capabilities/compiled.rs`

`crates/oxide-agent-core/src/agent/prompt/composer.rs`

`crates/oxide-agent-core/Cargo.toml`

Also inspect and modify if applicable:

`crates/oxide-agent-core/src/agent/providers/delegation.rs`

`crates/oxide-agent-core/src/agent/providers/manager_control_plane/mod.rs`

`crates/oxide-agent-core/src/agent/providers/manager_control_plane/agent_controls.rs`

Docs/env:

`.env.example`

Relevant README/local dev doc.

Tests/snapshots:

`crates/oxide-agent-core/tests/modular_registry_snapshots.rs`

`crates/oxide-agent-core/tests/snapshot_prompts.rs`

Provider-local unit tests in `crawl4ai_markdown.rs`.

Possibly add a focused snapshot file for tool definition stability.

### Success output contract

Return one compact, bounded object. The exact shape:

```json
{
  "provider": "crawl4ai_markdown",
  "url": "<input_url>",
  "final_url": "<final_url_or_null>",
  "status_code": 200,
  "success": true,
  "markdown_kind": "raw_markdown",
  "markdown": "<bounded markdown>",
  "truncated": false,
  "chars": 12345,
  "elapsed_ms": 1234
}
```

Rules:

1. `provider` is always `crawl4ai_markdown`.

2. `url` is the validated input URL.

3. `final_url` is included if Crawl4AI returns it.

4. `status_code` is included if available.

5. `markdown_kind` is one of:

   `raw_markdown`

   `markdown_with_citations`

   `fit_markdown`

6. `markdown` is trimmed and bounded.

7. `truncated` is true only if markdown was char-truncated.

8. `chars` is the returned markdown char count after truncation marker handling.

9. `elapsed_ms` is optional but allowed. It is runtime output, not prompt prefix.

Do not include Crawl4AI server version, health status, browser pool status, proxy status, request id, Docker tag, cache hit/miss, raw HTML, cleaned HTML, response headers, screenshots, PDF, links, or media in success output.

### Failure output contract

Failure object:

```json
{
  "provider": "crawl4ai_markdown",
  "error_kind": "crawl4ai_unavailable",
  "url": "<input_url_or_null>",
  "host": "<target_host_or_null>",
  "crawl4ai_base_url_host": "<base_url_host>",
  "status_code": null,
  "retryable": true,
  "provider_unavailable": true,
  "message": "<short bounded message>",
  "response_tail": "<optional bounded tail>"
}
```

Failure taxonomy:

`invalid_arguments`

Malformed JSON or invalid arg types.

`unsupported_url`

Unsupported scheme, no host, media/local/raw/data URL.

`ssrf_blocked`

Initial URL host/IP/DNS result is unsafe.

`dns_failed`

DNS preflight failed.

`crawl4ai_unavailable`

Health failed, connection refused, base URL unreachable.

`crawl4ai_auth_failed`

Crawl4AI returned 401/403 due to token/auth config.

`crawl4ai_http_status`

Crawl4AI REST returned non-2xx.

`crawl_failed`

Crawl4AI result has `success=false`.

`unexpected_result_count`

REST response did not contain exactly one result.

`parse_error`

REST JSON shape could not be parsed.

`timeout`

Health or crawl request timed out.

`cancelled`

Invocation cancellation token fired.

`response_too_large`

Crawl4AI JSON response exceeded body limit.

`final_url_blocked`

Crawl4AI final URL is unsafe.

`network`

Generic retryable transport error.

`internal`

Fallback only.

Retryable rules:

Retryable true for `crawl4ai_unavailable`, `timeout`, selected `network`, 429/5xx from Crawl4AI.

Retryable false for validation, SSRF, auth, parse, final URL blocked, unsupported URL, and 4xx except 429.

### REST endpoint

Use:

`POST /crawl`

Use health:

`GET /health`

Do not use:

`/md`

`/execute_js`

`/screenshot`

`/pdf`

`/crawl/stream`

`/crawl/job`

`/llm/job`

webhooks

MCP endpoints

Crawl4AI docs expose `/execute_js`, screenshot, PDF, hooks, jobs, streaming, and MCP capabilities; those are exactly why Oxide must keep the adapter narrow. ([Crawl4AI Documentation][1])

### Health behavior

1. Tool registration depends only on feature/profile/static module config.

2. Health is runtime-only.

3. Health failure returns structured failure.

4. Health failure does not remove tool.

5. Health result does not enter system prompt.

6. Health result does not change schema.

7. Health result does not change tool description.

8. No startup blocking on Crawl4AI.

For v1, skip provider-constructor health check. A constructor async health probe would complicate initialization for little value. Execute-time short health check is enough.

### Rollback plan

Fast rollback:

1. Build without `tool-crawl4ai-markdown`.

2. Or disable module `tool/crawl4ai-markdown` via existing module runtime config.

3. Leave Crawl4AI container stopped; Oxide should remain healthy.

4. Revert any profile composition change if the feature was added to active profiles.

5. No database migration, no persistent state, no cache cleanup.

6. Prompt/tool cache returns to previous surface when the feature/profile change is reverted.

---

## Cache-hit checklist

1. Tool name is exactly `crawl4ai_markdown`.

2. Tool schema is static and small.

3. Tool description is static.

4. Tool registration is feature/profile/module-config based, not health based.

5. No dynamic schema from env.

6. No dynamic prompt from env.

7. No Crawl4AI base URL in prompt.

8. No Crawl4AI health status in prompt.

9. No Crawl4AI version in prompt.

10. No URL, date, request id, crawl stats, browser pool status, proxy status, or cache state in cacheable system prefix.

11. Prompt update is one short static workflow sentence only.

12. Registry ordering is left to existing deterministic registry model.

13. Missing service returns structured `provider_unavailable`.

14. Dead service does not remove the tool.

15. Snapshot test locks tool schema and description.

16. Prompt snapshot confirms no dynamic service state.

17. Existing active profiles are not modified unless the owner accepts one intentional cache-surface rollout.

18. Crawl4AI content cache is documented as separate from LLM provider prompt cache.

19. Runtime output may include `elapsed_ms`, status, and markdown metadata; system prompt must not.

20. No per-request dynamic tool availability.

---

## Things explicitly not to do

Do not build a crawler platform.

Do not implement deep crawl.

Do not add multi-URL crawling.

Do not add browser automation API.

Do not add arbitrary JavaScript.

Do not expose Crawl4AI hooks.

Do not use `/execute_js`.

Do not use screenshots.

Do not use PDF export.

Do not use Crawl4AI LLM extraction.

Do not pass arbitrary Crawl4AI config from LLM args.

Do not add headers/cookies/proxy/user-agent/session/tool args.

Do not add Python code to Oxide Agent.

Do not run Python subprocesses from Oxide Agent.

Do not embed Python with pyo3.

Do not add MCP for this integration.

Do not manage Docker container lifecycle from Oxide Agent.

Do not add persistent Oxide-side web cache.

Do not add queue/storage/browser pool in Oxide.

Do not add proxy rotation subsystem in Oxide.

Do not add CAPTCHA solving.

Do not add credential/session manager.

Do not expose Crawl4AI API publicly without auth/network controls.

Do not use `latest` image tag in production.

Do not use Crawl4AI versions below `0.8.0`.

Do not change tool availability based on live health.

Do not put health/tool output/cache state into cacheable system prompt.

Do not make this “smart” before it is reliable. The first implementation should be boring, bounded, and easy to delete.

[1]: https://docs.crawl4ai.com/core/self-hosting/ "Self-Hosting Guide - Crawl4AI Documentation (v0.8.x)"
[2]: https://docs.crawl4ai.com/api/parameters/ "Browser, Crawler & LLM Config - Crawl4AI Documentation (v0.8.x)"
[3]: https://docs.crawl4ai.com/core/cache-modes/?utm_source=chatgpt.com "Cache Modes - Crawl4AI Documentation (v0.8.x)"
[4]: https://docs.crawl4ai.com/api/crawl-result/?utm_source=chatgpt.com "CrawlResult - Crawl4AI Documentation (v0.8.x)"
[5]: https://hub.docker.com/r/unclecode/crawl4ai/tags "unclecode/crawl4ai - Docker Image"
[6]: https://github.com/advisories/GHSA-5882-5rx9-xgxp?utm_source=chatgpt.com "Crawl4AI is Vulnerable to Remote Code Execution in ..."
