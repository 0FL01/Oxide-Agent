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
- Quick facts/news -> web_search (direct tool)
- Read article -> **DELEGATE** via `delegate_to_sub_agent` using `web_markdown`
- JS-heavy SPA sites -> **DELEGATE** via `delegate_to_sub_agent` using `deep_crawl`
- Save for later/archive -> **DELEGATE** via `delegate_to_sub_agent` using `web_pdf`

## Mandatory Delegation for Crawl4AI
All Crawl4AI tools (`deep_crawl`, `web_markdown`, `web_pdf`) MUST be used via sub-agent. Direct calls are blocked.
1. Use `delegate_to_sub_agent`.
2. Add the specific tool name to the `tools` array.
3. Provide a clear task for the sub-agent.

**Example**:
```json
{
  "task": "Extract markdown content from https://example.com/article",
  "tools": ["web_markdown"]
}
```
