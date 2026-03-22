# 1. Goal

This document describes a portable sub-agent architecture for a Rust agent framework modeled after Codex. It is adapted for a framework where tools may run in parallel inside an agent.

The target behavior is:

- the main agent can spawn sub-agents as background async tasks;
- each sub-agent owns its own session, history, status, and inbox;
- the parent can create a child, send more input, wait for completion, or stop it;
- the system tracks the parent-child tree;
- child completion is signaled back to the parent automatically;
- tool calls inside an agent can run concurrently when the tool is marked parallel-safe.

This gives the same operational flow style as Codex:

- spawn does not block the parent;
- wait is only used when the result is immediately needed;
- the parent continues useful work while children run in the background;
- if the model emits a batch of independent tool calls, they execute concurrently.

---

# 2. Sync or Async

Recommended model:

- `spawn_agent(...).await` is an async API;
- the agent itself runs in the background via `tokio::spawn`;
- `wait_agent(...).await` is async and non-blocking;
- communication happens through channels;
- agent status is broadcast through `watch`.

In practice:

- `spawn_agent(...).await` performs async initialization, registration, and runtime startup;
- after that, the child lives independently;
- the parent is not blocked;
- `wait_agent(...).await` waits on state changes rather than polling in a busy loop.
