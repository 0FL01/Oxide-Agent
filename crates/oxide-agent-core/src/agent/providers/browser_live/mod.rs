//! Browser Live Agent sidecar contract.
//!
//! Runtime tool registration and browser loop execution are added by later
//! checkpoints. CP-5 adds a test-only fake sidecar behind `cfg(test)` so unit
//! tests can cover browser lifecycle code without Chromium or external services.
//!
//! Documentation: `docs/browser-live.md`

/// Action execution planning from direct `BrowserAction` inputs.
pub mod actions;
/// Typed HTTP client for the browser sidecar REST API.
pub mod artifacts;
/// Task-local browser session state and screenshot ring-buffer.
pub mod client;
/// Error types and retry classification for browser sidecar operations.
pub mod error;
/// Browser Live MVP metrics and structured logging.
pub mod metrics;
/// Browser Live MVP security policy helpers.
pub mod policy;
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
pub use metrics::{BrowserMetricsCollector, BrowserMetricsSnapshot};
pub use policy::{BrowserPolicyAuditEvent, BrowserPolicyError};
pub use session::{BrowserFrame, BrowserSessionState};
pub use tools::BrowserLiveProvider;
pub use types::{
    BrowserAction, BrowserObservation, BrowserProfile, CreateSessionRequest, ScreenshotArtifact,
    SidecarErrorBody, Viewport,
};
