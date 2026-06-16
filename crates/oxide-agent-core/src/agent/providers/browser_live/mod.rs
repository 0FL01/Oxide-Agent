//! Browser Live Agent sidecar contract.
//!
//! Runtime tool registration and browser loop execution are added by later
//! checkpoints. CP-5 adds a test-only fake sidecar behind `cfg(test)` so unit
//! tests can cover browser lifecycle code without Chromium or external services.

/// Typed HTTP client for the browser sidecar REST API.
pub mod artifacts;
/// Task-local browser session state and screenshot ring-buffer.
pub mod client;
/// Error types and retry classification for browser sidecar operations.
pub mod error;
/// MiMo screenshot decision caller.
pub mod mimo;
/// Strict BrowserDecision parser and validation.
pub mod parser;
/// Prompt construction for Browser Live MiMo decisions.
pub mod prompt;
/// Browser session state model.
pub mod session;
#[cfg(test)]
pub(crate) mod test_support;
/// Native tool executors for Browser Live.
pub mod tools;
/// Request, response, artifact, and event contract types.
pub mod types;

pub use artifacts::{BrowserArtifactPurpose, BrowserArtifactSettings};
pub use client::{BrowserSidecar, BrowserSidecarClient, BrowserSidecarTimeouts, IdempotencyKey};
pub use error::BrowserSidecarError;
pub use mimo::BrowserMimoDecider;
pub use session::{BrowserFrame, BrowserSessionState};
pub use tools::BrowserLiveProvider;
pub use types::{
    BrowserAction, BrowserDecision, BrowserDecisionAction, BrowserDecisionRisk, BrowserObservation,
    BrowserSensitiveAction, CreateSessionRequest, ScreenshotArtifact, SidecarErrorBody, Viewport,
};
