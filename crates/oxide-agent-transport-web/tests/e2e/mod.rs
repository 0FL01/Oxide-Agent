//! E2E tests for the web transport.
//!
//! Tests the full agent execution pipeline with a scripted LLM provider
//! to measure application-level latency without depending on real LLM APIs.

mod compaction_regression_tests;
mod delegation_tests;
mod helpers;
mod integration_tests;
mod live_zai_audit_tests;
mod providers;
mod session_tests;
mod setup;
mod sse_tests;
mod tool_latency_tests;
