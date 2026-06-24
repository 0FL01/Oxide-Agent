//! Shared OpenAI-compatible Chat Completions wire path.
//!
//! Request, response, streaming, and profile responsibilities are split here
//! so provider wrappers delegate to a single parity-tested path.

pub(crate) mod client;
pub(crate) mod profile;
pub(crate) mod request;
pub(crate) mod response;
pub(crate) mod streaming;
