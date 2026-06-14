//! Mistral AI LLM provider
//!
//! The implementation has been migrated to [`openai_base::OpenAIBaseProvider`]
//! with a Mistral-specific profile. This module only provides the
//! [`MistralProviderModule`] for backward-compatible route registration.

pub(crate) mod module;
pub(crate) use module::MistralProviderModule;
