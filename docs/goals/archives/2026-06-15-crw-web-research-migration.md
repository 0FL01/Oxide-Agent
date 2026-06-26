# Archived CRW web research migration

This archived goal captured the earlier CRW migration work. It has been superseded by the unified indexed-search architecture.

Current state:

- `web_search` is the only indexed-search tool exposed to agents.
- CRW, Tavily, and Brave are private backends selected by the unified provider from configured endpoint/API-key environment.
- CRW scrape remains available only as a rendered-fetch backend for `web_crawler`.
- The previous split search modules, duplicate-name guards, and boolean enable switches were removed.

This archive keeps only the durable outcome because the step-by-step historical notes referenced retired module names and environment switches that must not remain as searchable contracts.
