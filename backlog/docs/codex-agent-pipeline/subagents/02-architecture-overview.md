# 2. Architecture Overview

```text
Parent Agent
  |
  | spawn_agent()
  v
AgentControl
  |
  |- reserves slot / nickname / path
  |- creates child session
  |- registers child in ThreadManager
  |- starts child runtime via tokio::spawn
  |- attaches completion watcher
  `- sends initial input into child mailbox
        |
        v
   Child Agent Runtime
        |
        |- reads Op messages from mpsc inbox
        |- runs turn loop
        |- calls LLM
        |- executes tool calls
        |- may spawn further agents
        `- publishes status through watch
```

---

# 3. Design Principles

- control plane is separate from execution plane;
- each agent is an actor with its own mailbox;
- shared state is minimized;
- the parent learns about child completion through a watcher;
- tool calls may execute in parallel when allowed by tool capabilities.
