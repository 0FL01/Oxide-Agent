---
name: task-planning
description: Multistep task planning and status management via write_todos.
triggers: [plan, step, research, compare, analysis, todo list, tasks]
allowed_tools: [write_todos]
weight: high
---
## Task Management:
- **write_todos**: create or update the task list for the current request
  - MUST use for complex requests requiring multiple steps (research, comparison, analysis)
  - Create a plan BEFORE starting work
  - Update task statuses as they are completed
  - DO NOT give a final answer until ALL tasks are completed
  - Statuses: `pending`, `in_progress`, `completed`, `cancelled`
  - ONLY ONE task can be `in_progress` at a time

## CRITICAL: Task Planning

### When to use write_todos:
1. **Research requests**: "Compare X and Y", "Find the best way...", "Analyze..."
2. **Multistep tasks**: any task requiring more than one tool
3. **Information gathering**: "Tell me about the current situation with...", "What's the news on..."

### Example of correct usage:
```json
{
  "todos": [
    {"description": "Search for info about X", "status": "in_progress"},
    {"description": "Read documentation", "status": "pending"},
    {"description": "Analysis and comparison", "status": "pending"},
    {"description": "Formulate conclusions", "status": "pending"}
  ]
}
```

### After completing each step:
1. Update the status of the completed task to `completed`
2. Update the status of the next task to `in_progress`
3. Continue working
