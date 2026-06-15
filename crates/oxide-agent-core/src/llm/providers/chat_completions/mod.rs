//! Shared OpenAI-compatible Chat Completions wire path.
//!
//! This module is intentionally introduced as a compile-checked skeleton before
//! production providers are routed through it. Request, response, streaming, and
//! profile responsibilities are split here so the migration can move behavior in
//! small parity-tested steps.

#![allow(dead_code)]

pub(crate) mod client;
pub(crate) mod profile;
pub(crate) mod request;
pub(crate) mod response;
pub(crate) mod streaming;
