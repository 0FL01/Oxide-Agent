# 17. Bottlenecks and Recommended Improvements

## Bottlenecks and Risks

Even a good design has limits:

- the shared `ThreadManager` can become a contention point;
- too many live agents can create memory and context pressure;
- parallel tools may saturate I/O or external services;
- if the model calls `wait_agent` too often, it degrades the async flow;
- shared workspace scenarios need strict write-scope discipline.

---

## Recommended Improvements

For a stronger production design, add:

- per-agent `max_parallel_tools`;
- per-tool `supports_parallel`;
- per-tool `max_concurrency`;
- `agent_max_depth`;
- `agent_max_threads`;
- automatic completion notification to parents;
- disjoint write-scope policy for coding sub-agents;
- telemetry for:
  - spawn count,
  - active agents,
  - wait frequency,
  - tool batch width,
  - average parallel tool fanout.
