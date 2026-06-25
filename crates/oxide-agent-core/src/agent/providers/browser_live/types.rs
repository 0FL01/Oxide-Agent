//! Re-export of shared REST contract types from `oxide-browser-contracts`.
//!
//! The canonical type definitions live in `oxide-browser-contracts` so the
//! native browser sidecar binary and this Oxide client share one source of
//! truth — eliminating the contract-drift class of bugs.

pub use oxide_browser_contracts::*;
