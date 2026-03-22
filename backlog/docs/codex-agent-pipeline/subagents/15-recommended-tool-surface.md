# 15. Recommended Tool Surface

Recommended control tools:

- `spawn_agent`
  - input: `message`, `task_name`, `agent_type`, `model`, `fork_context`
  - output: `agent_id`, `task_name`, `nickname`

- `send_input`
  - input: `target`, `message`, `interrupt`
  - output: `submission_id`

- `wait_agent`
  - input: `targets[]`, `timeout_ms`
  - output: `status map`, `timed_out`

- `close_agent`
  - input: `target`
  - output: `ok`

This is very close to the operational surface used by Codex and works well in practice.
