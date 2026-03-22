# 16. Parallel Tools Rules

Given that your framework already benefits from parallel tools, the following rules are recommended:

- the model may emit a batch of independent tool calls in one turn;
- the scheduler must know which tools are parallel-safe;
- exclusive tools must take a global write lock, while parallel-safe tools take a shared read lock.

## Examples

Parallel-safe (acquire shared read lock):

- `read_file`
- `grep`
- `http_get`
- `search`
- `stat`

Exclusive (acquire write lock):

- `apply_patch`
- `db_write`
- `git_commit`
- `close_agent_tree`

## Default Rule

If unsure whether a tool supports parallelism, start with `supports_parallel = false`. It is easier to enable parallelism later than to debug race conditions from premature enabling.
