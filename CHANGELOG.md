# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-01-24

### Added

- **Comprehensive Testing Infrastructure**:
  - Added `mockall` (0.14.0) dependency for trait-based mocking
  - Added `insta` (1.46.1) dependency for snapshot testing
  - New `testing.rs` module with helper functions (`mock_llm_simple()`, `mock_storage_noop()`)
  - Hermetic agent tests (`tests/hermetic_agent.rs` - 236 lines) with SuccessMock and FailureMock
  - Property-based fuzzing tests (`tests/proptest_recovery.rs` - 66 lines) for XML/JSON recovery
  - Snapshot tests (`tests/snapshot_prompts.rs` - 26 lines) for prompt regression testing
- **Mock Implementations**:
  - `MockLlmProvider` for testing LLM interactions
  - `MockStorageProvider` for testing storage operations
- **Development Tooling**:
  - OpenCode plugin (`rust-git-guard.ts`) for automated code review checks

### Changed

- **Storage Provider**: Refactored to support mock implementations and improve testability
- **Testing Strategy**: Introduced hermetic testing to reduce external dependencies in tests
- **Documentation**: Updated `AGENTS.md` with testing infrastructure documentation (+60 lines)

### Fixed

- **Clippy Linting**: Resolved clippy conflicts in testing.rs
- **Code Quality**: Removed unused imports in core and testing modules

## [0.3.0] - 2026-01-21

### Added

- **Workspace Architecture**: Complete refactor into modular workspace with four crates:
  - `oxide-agent-core` - Domain logic, LLM integrations, hooks, skills, storage
  - `oxide-agent-runtime` - Session orchestration, execution cycle, tool providers, sandbox
  - `oxide-agent-transport-telegram` - Telegram transport layer (teloxide integration)
  - `oxide-agent-telegram-bot` - Binary entry point and configuration
- **Crawl4AI Integration**: Deep web crawling provider with:
  - Markdown extraction from web pages
  - PDF parsing and conversion
  - Advanced crawling strategies (lazy loading, virtual scroll, undetected browser)
  - File downloading capabilities
  - Session management and proxy security
  - Identity-based crawling with hooks
- **Advanced Hook System**:
  - `SearchBudgetHook` - Prevents infinite loops in tool calls
  - `DelegationGuard` - Controls sub-agent delegation behavior
  - `SoftTimeoutReportHook` - Provides detailed timeout reporting
  - `CompletionCheckHook` - Validates task completion
- **Loop Detection System**: Agent loopback detection to prevent hallucination loops
- **Narrator System**: Enhanced dialogue management for better context
- **Progress Rendering Abstraction**: Transport-agnostic progress reporting
- **ZAI Provider Enhancements**: Custom API base URL support, improved SDK integration
- **Runtime Debug Switch**: `DEBUG_MODE` environment variable for dynamic verbosity
- **Localization**: Multi-language agent responses support

### Changed

- **Project Structure**: All source files moved from root `src/` to crate-specific locations
- **Workspace Dependency Management**: Centralized dependency management
- **Transport Layer Decoupling**: Progress rendering separated from Telegram-specific code
- **Logging Strategy**: Optimized verbosity for containerized environments
- **Docker Configuration**: Updated for workspace structure with multi-stage builds
- **Documentation**: Overhauled `AGENTS.md` and added 100+ docs

### Fixed

- **ZAI Tool Call Streaming**: Resolved argument concatenation issues
- **Concurrency Safety**: Proper error handling for embedding Mutex poisoning
- **Token Calculation**: Improved accuracy across providers
- **Progress Reporting**: Fixed race conditions in multi-threaded updates

### Removed

- **Monolithic Structure**: Removed single-crate architecture
- **Legacy Provider Files**: Consolidated into workspace crates

### Breaking Changes

- **Import Paths**: All imports must be updated to new crate structure (`oxide_agent_core::*`, etc.)
- **Configuration**: New required environment variables (`AGENT_TIMEOUT_SECS`, `SEARCH_PROVIDER`)
- **Dockerfile**: References to `src/` must be updated to workspace paths
- **CI/CD Pipelines**: Build paths changed to workspace-based
- **External Scripts**: Any scripts referencing old structure must be updated

### Planned

- **ZAI Refactor**: Full migration to `zai-rs` SDK with GLM-4.7/4.5 support
- **Sandbox Disk Quotas**: XFS-based disk space limits
- **Additional Transports**: Discord and Slack integrations

## [0.2.0] - 2026-01-16

### Added

- **Dynamic Model Configuration**: Full environment-driven model selection for chat, agent, sub-agent, media, narrator, and embedding models via `CHAT_MODEL_ID`, `AGENT_MODEL_ID`, `NARRATOR_MODEL_ID`, etc.
- **OpenRouter Native Tool Support**: Structured messages and tool calling capabilities for the OpenRouter provider.
- **Universal Embeddings Provider**: Replaced Mistral-specific implementation with a provider-agnostic embedding system supporting Mistral and OpenRouter, with auto-detection of embedding dimensions.
- **Mistral Structured Output**: Support for structured output responses from Mistral models.
- **Agent Model Display**: Current agent model is now shown in the welcome message for transparency.
- **Unified Destructive Action Confirmation**: Consistent confirmation prompts for memory clear and container recreation operations.

### Changed

- **LLM Provider Architecture**: Refactored monolithic `providers.rs` (1138 lines) into modular per-provider files under `src/llm/providers/` (Gemini, Groq, Mistral, OpenRouter, ZAI).
- **ZAI Provider Simplification**: Streamlined agent request parameters and improved request body construction.
- **Configuration Module**: Extended `Settings` with comprehensive model configuration fields and validation.
- **CI/CD Pipeline**: Updated workflow with new model configuration environment variables.
- **Documentation**: Synchronized `AGENTS.md` with actual project structure; updated READMEs with supported providers and recommended models.

### Fixed

- **ZAI API Compatibility**: Resolved multiple issues with ZAI provider including unsupported `tool_choice` parameter, empty tools array handling, and temperature serialization (f32 → f64).
- **Code Block Regex**: Loosened pattern matching to allow trailing whitespace in code blocks.
- **Integration Tests**: Updated tests to match new dynamic model configuration API.
- **Embedding Model Default**: Corrected Mistral embedding model to `codestral-embed` in configuration example.

### Removed

- **Dead Code**: Removed unused native tool calling code from MistralProvider.
- **Legacy Provider File**: Deleted monolithic `src/llm/providers.rs` in favor of modular structure.

## [0.1.0] - 2026-01-11

Initial release.