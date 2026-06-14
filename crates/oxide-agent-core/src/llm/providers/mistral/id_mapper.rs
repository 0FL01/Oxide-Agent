//! Re-export of `ToolCallIdMapper` from the shared OpenAI-compatible layer.
//!
//! The implementation was moved to
//! `openai_base::tool_ids` during the profile migration.
//! Mistral-specific code continues to reference `mistral::id_mapper::ToolCallIdMapper`
//! for compatibility until the provider implementation is fully removed.

pub(crate) use crate::llm::providers::openai_base::tool_ids::ToolCallIdMapper;
