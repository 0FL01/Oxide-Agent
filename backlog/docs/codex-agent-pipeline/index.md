# Sub-Agents in Rust Agent Framework

## Overview

This document describes a portable sub-agent architecture for a Rust agent framework modeled after Codex. It is adapted for a framework where tools may run in parallel inside an agent.

## Navigation

### Foundational

- [01 - Goals and Async Model](subagents/01-goals-and-async-model.md)
- [02 - Architecture Overview](subagents/02-architecture-overview.md)
- [03 - Dependencies](subagents/03-dependencies.md)

### Core Data Structures

- [04 - Base Types](subagents/04-base-types.md)
- [05 - Thread Manager (Registry)](subagents/05-thread-manager.md)

### Tools Layer

- [06 - Tool Abstraction](subagents/06-tool-abstraction.md)
- [07 - Parallel Tool Runtime](subagents/07-parallel-tool-runtime.md)

### Agent Layer

- [08 - Session and Model Layer](subagents/08-session-and-model.md)
- [09 - Agent Runtime](subagents/09-agent-runtime.md)
- [10 - AgentControl: Control Plane](subagents/10-agent-control.md)
- [11 - Completion Watcher](subagents/11-completion-watcher.md)

### Examples and Integration

- [12 - Examples: ModelFactory, Tools, Usage](subagents/12-examples.md)
- [13 - Framework Construction](subagents/13-framework-construction.md)
- [14 - Integration into Real LLM Loop](subagents/14-integration-llm-loop.md)

### Production Guidance

- [15 - Recommended Tool Surface](subagents/15-recommended-tool-surface.md)
- [16 - Parallel Tools Rules](subagents/16-parallel-tools-rules.md)
- [17 - Bottlenecks and Recommended Improvements](subagents/17-bottlenecks-and-improvements.md)
- [18 - Practical Conclusion](subagents/18-conclusion.md)

## Quick Summary

The architecture is built on these pillars:

| Concept               | Mechanism                     |
| --------------------- | ----------------------------- |
| Agent lifecycle       | `tokio::spawn` + mpsc inbox   |
| Status updates        | `watch::channel`              |
| Orchestration         | `AgentControl`                |
| Parent notification   | Background completion watcher |
| Parallel tools        | `RwLock` gate + `Semaphore`   |
| Max depth             | Reservation + depth counter   |
| Max concurrent agents | `Semaphore`                   |

Each sub-agent is an independent async actor. The parent spawns, sends input, waits, or closes. Completion is signaled back automatically via a watcher.
