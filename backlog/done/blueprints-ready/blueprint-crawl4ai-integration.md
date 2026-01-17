# Blueprint: Crawl4AI Integration

**Feature:** Web crawling via Crawl4AI sidecar service  
**Status:** Draft  
**Created:** 2026-01-16  
**Architecture:** Sidecar Docker container  

---

## Overview

Интеграция Crawl4AI как альтернативного провайдера веб-поиска. Crawl4AI работает как sidecar-контейнер и предоставляет инструменты для глубокого краулинга JS-рендеренных страниц, извлечения markdown и экспорта PDF.

**Ключевые решения:**
- Tavily и Crawl4AI взаимоисключающие (compile-time check)
- Crawl4AI как Docker sidecar на порту 11235
- 3 инструмента: `deep_crawl`, `web_markdown`, `web_pdf`
- Memory limit: 4GB

---

## Phase 0: Feature Flags Setup [ ]

**Goal:** Настроить взаимоисключающие features для Tavily и Crawl4AI.

**Resource Context:**
- 📄 `Cargo.toml`
- 📄 `src/agent/providers/mod.rs`

**Steps:**
1. [ ] В `Cargo.toml` добавить feature `crawl4ai = []` (без зависимостей, использует существующий `reqwest`)
2. [ ] В `src/agent/providers/mod.rs` добавить compile_error для взаимоисключения features
3. [ ] Добавить условный экспорт модуля `crawl4ai`
4. [ ] **QA:** `cargo check --features tavily` и `cargo check --features crawl4ai` должны работать отдельно
5. [ ] **QA:** `cargo check --features tavily,crawl4ai` должен выдать compile_error

> [!NOTE]
> Cargo не поддерживает exclusive features напрямую, поэтому используем `compile_error!` макрос.

---

## Phase 1: Docker Infrastructure [ ]

**Goal:** Настроить Crawl4AI как sidecar-сервис в docker-compose.

**Resource Context:**
- 📄 `docker-compose.yml`
- 📄 `.env.example`
- 📚 **Docs:** Crawl4AI self-hosting guide — `https://docs.crawl4ai.com/core/self-hosting/`

**Steps:**
1. [ ] **Verify API:** Использовать `tavily_extract` для проверки актуальных endpoints Crawl4AI (`/crawl`, `/md`, `/pdf`)
2. [ ] Убрать `network_mode: "host"` из `oxide_agent` service
3. [ ] Добавить bridge network `oxide_network`
4. [ ] Добавить сервис `crawl4ai`:
   - image: `unclecode/crawl4ai:latest`
   - networks: `oxide_network`
   - volumes: `/dev/shm:/dev/shm`
   - memory limit: 4G
   - healthcheck на `/health`
5. [ ] Добавить `depends_on` с `condition: service_healthy` в `oxide_agent`
6. [ ] Добавить environment variable `CRAWL4AI_URL=http://crawl4ai:11235`
7. [ ] Обновить `.env.example` с новыми переменными
8. [ ] **QA:** `docker compose config` для валидации YAML

> [!IMPORTANT]
> При изменении network_mode убедиться, что Docker socket монтируется через volume (для sandbox).

**Docker Compose Structure:**
```
services:
  oxide_agent:
    networks: [oxide_network]
    depends_on:
      crawl4ai:
        condition: service_healthy
    environment:
      - CRAWL4AI_URL=http://crawl4ai:11235

  crawl4ai:
    image: unclecode/crawl4ai:latest
    networks: [oxide_network]
    volumes:
      - /dev/shm:/dev/shm
    deploy:
      resources:
        limits:
          memory: 4G
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:11235/health"]

networks:
  oxide_network:
    driver: bridge
```

---

## Phase 2: Configuration [ ]

**Goal:** Добавить конфигурацию Crawl4AI в Settings.

**Resource Context:**
- 📄 `src/config.rs`

**Steps:**
1. [ ] Добавить поля в struct `Settings`:
   - `crawl4ai_url: Option<String>`
   - `crawl4ai_timeout_secs: Option<u64>`
2. [ ] Добавить константу `CRAWL4AI_DEFAULT_TIMEOUT_SECS: u64 = 120`
3. [ ] Добавить функцию `get_crawl4ai_url() -> Option<String>`
4. [ ] Добавить функцию `get_crawl4ai_timeout() -> u64`
5. [ ] **QA:** `cargo check` без ошибок

---

## Phase 3: Crawl4AI Provider Implementation [ ]

**Goal:** Реализовать провайдер с 3 инструментами.

**Resource Context:**
- 📄 `src/agent/providers/tavily.rs` (reference implementation)
- 📄 `src/agent/provider.rs` (ToolProvider trait)
- 📚 **Docs:** Crawl4AI API endpoints:
  - `POST /crawl` — deep crawling
  - `POST /md` — markdown extraction  
  - `POST /pdf` — PDF export

**Steps:**
1. [ ] **Verify API Signatures:** Использовать `tavily_search` + `tavily_extract` для проверки:
   - Request/response format для `/crawl`
   - Request/response format для `/md`
   - Request/response format для `/pdf`
2. [ ] Создать файл `src/agent/providers/crawl4ai.rs`
3. [ ] Определить struct `Crawl4aiProvider`:
   - `base_url: String`
   - `client: reqwest::Client`
   - `timeout: Duration`
4. [ ] Реализовать `Crawl4aiProvider::new(base_url: &str) -> Self`
5. [ ] Определить argument structs:
   - `DeepCrawlArgs { urls: Vec<String>, max_depth: Option<u8> }`
   - `WebMarkdownArgs { url: String }`
   - `WebPdfArgs { url: String }`
6. [ ] Реализовать `ToolProvider` trait:
   - `name()` → `"crawl4ai"`
   - `tools()` → 3 ToolDefinition
   - `can_handle()` → match на имена
   - `execute()` → HTTP POST к endpoints
7. [ ] Обработка ошибок: возвращать user-friendly сообщения
8. [ ] **QA:** `cargo check --features crawl4ai`
9. [ ] **QA:** `cargo clippy --features crawl4ai`

> [!IMPORTANT]
> Перед реализацией `execute()` обязательно проверить актуальные API signatures через документацию, так как Crawl4AI активно развивается.

**Tool Definitions:**

| Tool | Description | Parameters |
|------|-------------|------------|
| `deep_crawl` | Deep crawl website with JS rendering | `urls: string[]`, `max_depth?: number` |
| `web_markdown` | Extract markdown from URL | `url: string` |
| `web_pdf` | Export webpage to PDF | `url: string` |

**API Request Examples:**

```json
// POST /crawl
{
  "urls": ["https://example.com"],
  "crawler_config": {
    "type": "CrawlerRunConfig",
    "params": {"cache_mode": "bypass"}
  }
}

// POST /md
{
  "url": "https://example.com",
  "f": "fit"
}

// POST /pdf
{
  "url": "https://example.com"
}
```

---

## Phase 4: Provider Registration [ ]

**Goal:** Зарегистрировать провайдер в executor и delegation.

**Resource Context:**
- 📄 `src/agent/executor.rs` (lines 104-156)
- 📄 `src/agent/providers/delegation.rs` (lines 100-120)

**Steps:**
1. [ ] В `executor.rs` добавить import под `#[cfg(feature = "crawl4ai")]`
2. [ ] В `executor.rs` добавить регистрацию после Tavily блока:
   ```rust
   #[cfg(feature = "crawl4ai")]
   if let Ok(url) = std::env::var("CRAWL4AI_URL") {
       if !url.is_empty() {
           registry.register(Box::new(Crawl4aiProvider::new(&url)));
       }
   }
   ```
3. [ ] В `delegation.rs` добавить аналогичную регистрацию в `build_sub_agent_registry()`
4. [ ] **QA:** `cargo check --features crawl4ai`

---

## Phase 5: Skill File Update [ ]

**Goal:** Обновить skill-файл для поддержки обоих провайдеров.

**Resource Context:**
- 📄 `skills/web-search.md`

**Steps:**
1. [ ] Обновить `allowed_tools` — добавить `deep_crawl`, `web_markdown`, `web_pdf`
2. [ ] Обновить `triggers` — добавить `crawl`, `extract`, `pdf`
3. [ ] Добавить секцию с описанием инструментов Crawl4AI
4. [ ] Добавить guidelines когда использовать какой инструмент

**Updated Content:**
```markdown
---
name: web-search
description: Search and extract information from the internet
triggers: [find, search, look up, current, news, docs, crawl, extract, pdf]
allowed_tools: [web_search, web_extract, deep_crawl, web_markdown, web_pdf]
weight: medium
---

## Web Search & Extraction

### Quick Search (Tavily):
- **web_search**: Search internet for news, facts, documentation
- **web_extract**: Extract content from URLs

### Deep Crawling (Crawl4AI):
- **deep_crawl**: Deep crawl with JS rendering for dynamic sites
- **web_markdown**: Fast markdown extraction from single URL
- **web_pdf**: Export webpage to PDF document

## Guidelines:
- Quick facts/news → web_search
- Read article → web_extract or web_markdown
- JS-heavy SPA sites → deep_crawl
- Save for later/archive → web_pdf
```

---

## Phase 6: Testing [ ]

**Goal:** Написать тесты для провайдера.

**Resource Context:**
- 📄 `src/agent/providers/crawl4ai.rs`
- 📄 `tests/` directory

**Steps:**
1. [ ] Добавить unit-тесты в `crawl4ai.rs`:
   - Test argument deserialization
   - Test URL construction
   - Test error message formatting
2. [ ] Создать `tests/crawl4ai_provider.rs`:
   - Test `can_handle()` для всех 3 инструментов
   - Test `tools()` возвращает 3 определения
3. [ ] **QA:** `cargo test --features crawl4ai`

---

## Phase 7: Documentation [ ]

**Goal:** Обновить документацию проекта.

**Resource Context:**
- 📄 `AGENTS.md`
- 📄 `.env.example`

**Steps:**
1. [ ] В `AGENTS.md` добавить `crawl4ai.rs` в структуру providers
2. [ ] В `.env.example` добавить комментарии про взаимоисключение Tavily/Crawl4AI
3. [ ] Добавить пример конфигурации для Crawl4AI

---

## Summary

| Phase | Files | Estimated LOC |
|-------|-------|---------------|
| 0. Feature Flags | 2 | ~15 |
| 1. Docker | 2 | ~35 |
| 2. Config | 1 | ~25 |
| 3. Provider | 1 (new) | ~180 |
| 4. Registration | 2 | ~20 |
| 5. Skill | 1 | ~25 |
| 6. Testing | 2 | ~60 |
| 7. Docs | 2 | ~15 |
| **Total** | **~13 files** | **~375 LOC** |

**Estimated Time:** 1.5-2 hours

---

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| network_mode change breaks Docker socket | Medium | High | Verify socket mount works with bridge network |
| Crawl4AI API changes | Low | Medium | Pin image version, verify docs before impl |
| Large PDF responses | Medium | Low | Limit response size, add timeout |
| Crawl4AI cold start slow | Low | Low | healthcheck with start_period: 40s |

---

## Acceptance Criteria

- [ ] `cargo build --features crawl4ai` succeeds
- [ ] `cargo build --features tavily,crawl4ai` fails with compile_error
- [ ] `docker compose up` starts both services
- [ ] Agent can use `deep_crawl` tool successfully
- [ ] Agent can use `web_markdown` tool successfully  
- [ ] Agent can use `web_pdf` tool successfully
- [ ] All tests pass
