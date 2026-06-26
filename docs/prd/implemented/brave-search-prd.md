# Superseded indexed-search PRD

This historical PRD has been superseded by the unified indexed-search design.

Current contract:

- Agents see one indexed search tool: `web_search`.
- The tool chooses among configured CRW, Tavily, and Brave backends internally.
- Backend availability is derived only from the current endpoint/API-key environment contract.
- CRW scraping is separate from indexed search and is used by rendered `web_crawler` paths.

The previous separate indexed-search module design was intentionally removed to avoid stale contracts and duplicate tool names.
