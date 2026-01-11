---
name: delegation_manager
description: Delegating grunt work (files, git, search) to a sub-agent.
triggers: [delegate, subagent, subtask, research, overview, comparison, dataset, git, clone, repo, scan, file reading, indexing, study]
allowed_tools: [delegate_to_sub_agent]
weight: high
---
## When to Delegate
- Voluminous research tasks (search, data collection, reading long materials)
- **File system work: cloning repositories, reading many files, grep search, code indexing**
- Draft stages: source aggregation, initial filtering, creating a draft list
- Parallel subtasks that can be separated from the main dialogue

## Why this is Important
**The sub-agent works in the SAME SANDBOX as you.**
All files downloaded or created by the sub-agent (e.g., via `git clone`) will be available to you at the same paths.
Delegate routine work (setup, exploration) to it to save your context and tokens.

## How to Formulate a Task
1. Briefly describe the goal and desired result format
2. Explicitly specify the list of allowed tools
3. Add clarifying context (if important), but without unnecessary history

## Example Call
```json
{
  "task": "Collect 5 relevant sources on topic X and briefly describe key facts",
  "tools": ["web_search", "web_extract"],
  "context": "Use sources no older than 12 months. Result — brief list with links."
}
```
