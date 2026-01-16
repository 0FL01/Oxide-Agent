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
- Quick facts/news -> web_search
- Read article -> web_extract or web_markdown
- JS-heavy SPA sites -> deep_crawl
- Save for later/archive -> web_pdf
