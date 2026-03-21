# 18. Practical Conclusion

To reproduce Codex-like behavior closely in your own Rust framework:

- model each sub-agent as its own async actor;
- make spawn an async setup API plus `tokio::spawn`;
- expose status via `watch::channel`;
- send commands via `mpsc::channel`;
- centralize orchestration in `AgentControl`;
- notify parents through a background completion watcher;
- execute tool batches through `FuturesUnordered`;
- separate parallel and exclusive execution with `RwLock` plus `Semaphore`.

This gives you:

- non-blocking orchestration;
- natural parent-child flow;
- strong control over resources;
- and acceleration from parallel tool execution inside agents.
