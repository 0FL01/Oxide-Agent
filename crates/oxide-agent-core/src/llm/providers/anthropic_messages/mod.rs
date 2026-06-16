//! Internal compatibility re-export for the renamed `providers::messages` module.
//!
//! Production code should import `providers::messages`; this module remains only
//! to avoid breaking any temporary internal references during the migration.

pub(crate) use super::messages::{ANTHROPIC_VERSION, MessagesClient, request, response};

pub(crate) type AnthropicProfile = super::messages::MessagesProfile;
