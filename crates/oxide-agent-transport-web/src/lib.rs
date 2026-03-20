//! Web transport for Oxide Agent — HTTP API for Agent Mode execution.
//!
//! ## Overview
//!
//! This crate provides a transport-agnostic HTTP layer that exposes Agent Mode
//! capabilities via a REST API. It is designed to be used in E2E tests and
//! benchmarks to measure application-level latency without depending on Telegram.
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
//!     +-- [InMemoryStorage] -- StorageProvider impl, no R2 required
//!     +-- [WebAgentTransport] -- AgentTransport impl, collects events in memory
//!     +-- [ScriptedLlmProvider] -- deterministic LLM for tests
//! ```
//!
//! ## API Contract
//!
//! ### Sessions
//!
//! - `POST /sessions` — create a new session, returns `{ "session_id": "uuid" }`
//! - `GET /sessions/{id}` — get session metadata (status, created_at)
//! - `DELETE /sessions/{id}` — destroy session
//!
//! ### Tasks
//!
//! - `POST /sessions/{id}/tasks` — submit task text, returns `{ "task_id": "uuid" }`
//!   The request body is plain text. Response is 202 Accepted with task_id.
//!
//! - `GET /sessions/{id}/tasks/{task_id}/progress` — current ProgressState
//!
//! - `GET /sessions/{id}/tasks/{task_id}/events` — SSE stream of AgentEvent
//!
//! - `POST /sessions/{id}/tasks/{task_id}/cancel` — cancel running task
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
//! The timeline is stored per (session_id, task_id) and accessible via
//! `GET /sessions/{id}/tasks/{task_id}/timeline`.

pub mod api;
pub mod in_memory_storage;
pub mod scripted_llm;
pub mod server;
pub mod session;
pub mod web_transport;

pub use api::*;
pub use scripted_llm::*;
pub use server::*;
