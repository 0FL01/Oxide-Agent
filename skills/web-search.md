---
name: web-search
description: Search and extract information from the internet for up-to-date data and documentation.
triggers: [find, search, look up, current, news, docs]
allowed_tools: [web_search, web_extract]
weight: medium
---
## Web (information search):
- **web_search**: search the internet for up-to-date information (news, facts, documentation). Parameters: query (string), max_results (1-10)
- **web_extract**: extract content from web pages to read articles and documentation. Parameters: urls (array of URLs)

## Important Rules:
- **WEB_SEARCH**: To get current information (news, events, documentation) — USE web_search instead of curl. It is faster and more efficient.
