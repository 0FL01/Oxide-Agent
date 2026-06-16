//! Browser Live Agent sidecar contract.
//!
//! Runtime tool registration and browser loop execution are added by later
//! checkpoints. CP-5 adds a test-only fake sidecar behind `cfg(test)` so unit
//! tests can cover browser lifecycle code without Chromium or external services.

/// Action execution planning from validated browser decisions.
pub mod actions;
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
/// Browser Live MVP security policy gates.
pub mod policy;
/// Prompt construction for Browser Live MiMo decisions.
pub mod prompt;
/// Deterministic recovery classification and bounded fallback planning.
pub mod recovery;
/// Browser session state model.
pub mod session;
#[cfg(test)]
pub(crate) mod test_support;
/// Native tool executors for Browser Live.
pub mod tools;
/// Request, response, artifact, and event contract types.
pub mod types;
/// Post-action visual verification helpers.
pub mod verification;

pub use artifacts::{BrowserArtifactPurpose, BrowserArtifactSettings};
pub use client::{BrowserSidecar, BrowserSidecarClient, BrowserSidecarTimeouts, IdempotencyKey};
pub use error::BrowserSidecarError;
pub use mimo::{BrowserDecisionEngine, BrowserMimoDecider};
pub use policy::{BrowserPolicyAuditEvent, BrowserPolicyError};
pub use session::{BrowserFrame, BrowserSessionState};
pub use tools::BrowserLiveProvider;
pub use types::{
    BrowserAction, BrowserDecision, BrowserDecisionAction, BrowserDecisionRisk, BrowserObservation,
    BrowserSensitiveAction, CreateSessionRequest, ScreenshotArtifact, SidecarErrorBody, Viewport,
};
