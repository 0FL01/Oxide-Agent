//! Browser Live Agent sidecar contract.
//!
//! This module intentionally contains only the typed sidecar API client and
//! wire types for the CP-4 checkpoint. Runtime tool registration and browser
//! loop execution are added by later checkpoints.

/// Typed HTTP client for the browser sidecar REST API.
pub mod client;
/// Error types and retry classification for browser sidecar operations.
pub mod error;
/// Request, response, artifact, and event contract types.
pub mod types;

pub use client::{BrowserSidecarClient, BrowserSidecarTimeouts, IdempotencyKey};
pub use error::BrowserSidecarError;
pub use types::{
    BrowserAction, BrowserObservation, CreateSessionRequest, ScreenshotArtifact, SidecarErrorBody,
    Viewport,
};
