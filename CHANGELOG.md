# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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