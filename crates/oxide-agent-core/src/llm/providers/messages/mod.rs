//! Shared Anthropic-compatible Messages wire helpers.
//!
//! Provides request body construction, message conversion, tool schema encoding,
//! response parsing, and usage extraction for the Anthropic Messages API format.
//! Used by `opencode_go` and `anthropic` providers.

pub(crate) mod client;
pub(crate) mod profile;
pub(crate) mod request;
pub(crate) mod response;

pub(crate) use client::MessagesClient;
pub(crate) use profile::MessagesProfile;

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";
