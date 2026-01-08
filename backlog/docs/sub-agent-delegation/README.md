# LLM Coder Documentation - Sub-Agents

This directory contains comprehensive documentation about the sub-agent architecture from OpenCode, designed to help LLM agents and developers understand and implement this pattern in their own applications.

## 📚 Documentation Files

### [llm_coder_guide.md](llm_coder_guide.md)

**Quick Reference Guide** - Practical guide for LLM coders

- When to use sub-agents
- Common patterns and examples
- Best practices
- Quick examples and workflows
- Performance tips

### [subagent_architecture.md](subagent_architecture.md)

**Complete Architecture Documentation** - Deep dive into the system

- Core concepts and design principles
- Session hierarchy and isolation
- Task tool implementation details
- Event-driven progress tracking
- Permission system
- Built-in agents
- Configuration options
- Rust implementation considerations

### [rust_examples.md](rust_examples.md)

**Rust Implementation Examples** - Code examples for Rust developers

- Agent registry implementation
- Session management
- Task tool implementation
- Event bus with tokio
- Permission system
- Command integration
- Complete working examples
- Testing patterns

### [api_reference.md](api_reference.md)

**API Quick Reference** - Complete API documentation

- All available functions and methods
- Data structures and types
- Helper functions
- Error handling
- Common patterns
- File paths and line numbers
- Migration checklist

## 🚀 Quick Start

### For LLM Agents

1. Read `llm_coder_guide.md` to understand when and how to use sub-agents
2. Reference `api_reference.md` for specific function signatures
3. Use examples from `subagent_architecture.md` for implementation patterns

### For Rust Developers

1. Review `subagent_architecture.md` for architectural concepts
2. Study `rust_examples.md` for implementation details
3. Use `api_reference.md` as a quick reference while coding

### For Understanding the System

1. Start with `subagent_architecture.md` for complete overview
2. Use `llm_coder_guide.md` for practical usage patterns
3. Reference `rust_examples.md` for specific implementation details

## 🎯 Key Concepts

### Sub-Agents

Specialized AI assistants that can be invoked by primary agents for specific tasks. They run in isolated child sessions with restricted permissions.

### Session Hierarchy

```
Parent Session (Primary Agent)
├── Child Session 1 (Subagent: explore)
├── Child Session 2 (Subagent: general)
└── Child Session 3 (Subagent: custom-agent)
```

### Task Tool

The primary mechanism for spawning and managing sub-agents. Creates child sessions, tracks progress, and returns results to parent.

### Permission System

Flexible rules-based system that controls what tools and operations are available in each session. Child sessions inherit restrictive permissions.

### Event Bus

Pub/sub system for real-time progress tracking and communication between sessions.

## 🔧 Built-in Agents

### Primary Agents

- **build** - Default agent with full access
- **plan** - Read-only agent for analysis

### Sub-Agents

- **general** - Complex multi-step tasks
- **explore** - Fast codebase exploration

## 💡 Usage Patterns

### Pattern 1: Explore Then Analyze

```
1. Call @explore to find files
2. Call @general to analyze findings
3. Present combined results
```

### Pattern 2: Parallel Execution

```
1. Launch multiple @explore subagents simultaneously
2. Wait for all to complete
3. Synthesize results
```

### Pattern 3: Chain Subagents

```
Parent → Child 1 (@explore) → Child 2 (@general)
```

## 📖 File Structure Reference

```
packages/opencode/src/
├── agent/
│   └── agent.ts              # Agent registry
├── tool/
│   ├── task.ts              # Task tool implementation
│   └── task.txt             # Task description
├── session/
│   ├── index.ts             # Session management
│   ├── prompt.ts            # Prompt processing
│   └── message-v2.ts        # Message system
├── permission/
│   └── next.ts              # Permission system
└── config/
    └── config.ts            # Configuration
```

## 🛠️ Tech Stack

### Original Implementation

- TypeScript
- Node.js
- Event-driven architecture
- Pub/sub pattern
- Zod for validation

### Rust Implementation

- Tokio async runtime
- Arc<RwLock> for thread safety
- tokio::sync::broadcast for events
- Serde for serialization
- Thiserror for error handling

## 📋 Key Takeaways

1. **Isolation**: Each subagent runs in isolated child session
2. **Security**: Permission system controls access at multiple levels
3. **Flexibility**: Custom subagents can be defined via JSON or Markdown
4. **Visibility**: Users can inspect all subagent sessions
5. **Progress**: Real-time tracking via event bus
6. **Parallelism**: Multiple subagents can run concurrently
7. **Recursion Prevention**: Child sessions cannot spawn other subagents

## 🚧 Implementation Checklist

### Core Components

- [ ] Agent registry with built-in agents
- [ ] Session manager with parent/child relationships
- [ ] Task tool for spawning subagents
- [ ] Permission system with pattern matching
- [ ] Event bus for progress tracking
- [ ] Message and part system

### Advanced Features

- [ ] Session navigation
- [ ] Progress metadata updates
- [ ] Custom agent configuration
- [ ] Command integration
- [ ] Error handling and recovery

### Testing

- [ ] Unit tests for each component
- [ ] Integration tests for workflows
- [ ] Permission testing
- [ ] Session hierarchy testing
- [ ] Event bus testing

## 📚 External Resources

### OpenCode

- Website: https://opencode.ai
- Documentation: https://opencode.ai/docs
- Discord: https://opencode.ai/discord
- GitHub: https://github.com/anomalyco/opencode

### Rust Ecosystem

- Tokio: https://tokio.rs
- Serde: https://serde.rs
- Thiserror: https://docs.rs/thiserror

## 🤝 Contributing

This documentation is generated from the OpenCode repository. To contribute improvements:

1. Edit the original source files in `/packages/opencode/src/`
2. Update documentation accordingly
3. Submit a pull request to https://github.com/anomalyco/opencode

## 📄 License

This documentation follows the same license as the OpenCode project.

---

**Last Updated**: 2025-01-08
**Source**: OpenCode Repository (https://github.com/anomalyco/opencode)
