---
name: web-search
description: Search and extract information from the internet
triggers: [find, search, look up, current, news, docs, extract]
allowed_tools: [web_search, web_extract, searxng_search, web_markdown]
weight: medium
---

## Web Search & Extraction

### Discovery
- **web_search**: Search internet for news, facts, documentation when Tavily is enabled
- **searxng_search**: Search internet through self-hosted SearXNG when enabled

### Reading Known URLs
- **web_extract**: Extract content from URLs when Tavily is enabled
- **web_markdown**: Fetch one known http/https URL and return Markdown. It does not crawl, execute JavaScript, or export PDFs.

## Guidelines:
- Search/discovery -> `web_search` or `searxng_search`
- Read a specific article/page -> `web_markdown`
- Use `web_markdown` only with a fully-qualified URL found by search or provided by the user.
- For JS-heavy SPA pages or PDF export, say the lightweight fetcher cannot provide browser/PDF capabilities instead of pretending to crawl.

## Delegation Example
```json
{
  "task": "Read https://example.com/article and extract the key claims with citations",
  "tools": ["web_markdown"],
  "context": "Return a concise bullet list with the source URL."
}
```
