//! Web transport for Oxide Agent — HTTP API for Agent Mode execution.
//!
//! ## Overview
//!
//! This crate provides a transport-agnostic HTTP layer that exposes Agent Mode
//! capabilities through the browser-facing `/api/v1` REST/SSE API.
//!
//! ## Architecture
//!
//! ```text
//! HTTP Request
//!     |
//!     v
//! [Axum Router] -- HTTP layer, session lifecycle, task submission
//!     |
//!     v
//! [WebSessionManager] -- orchestrates sessions using SessionRegistry
//!     |
//!     v
//! [AgentExecutor] -- core agent loop (from oxide-agent-core)
//!     |
//!     +-- [InMemoryStorage] -- StorageProvider impl for hermetic tests
//!     +-- [WebAgentTransport] -- AgentTransport impl, collects events in memory
//!     +-- [ScriptedLlmProvider] -- deterministic LLM for tests
//! ```
//!
//! ## Latency Measurement
//!
//! Each task records a `TaskTimeline` with the following milestones:
//!
//! - `http_received` — moment the HTTP request was processed by the router
//! - `session_ready` — ExecutorRwLock acquired, session prepared
//! - `first_thinking` — first AgentEvent::Thinking received
//! - `tool_calls` — list of (name, started_at, finished_at)
//! - `final_response` — moment AgentExecutionOutcome::Completed was produced
//! - `memory_persisted` — memory checkpoint saved to InMemoryStorage
//!
//! The timeline is stored per `(session_id, task_id)` for internal/test
//! inspection while the browser-facing API reads task records, persisted events,
//! and progress snapshots.

pub mod api;
pub mod auth;
pub mod in_memory_storage;
pub mod persistence;
pub mod scripted_llm;
pub mod server;
pub mod session;
pub mod web_transport;

pub use api::*;
pub use scripted_llm::*;
pub use server::*;
