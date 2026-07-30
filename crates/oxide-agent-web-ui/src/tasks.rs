pub(crate) mod activity;
pub(crate) mod composer;
mod delivered_files;
mod lightbox;
pub(crate) mod payload;
mod profile;
pub(crate) mod state;
mod streaming;
mod task_card;
mod tool_cards;
mod versions;
mod workspace;

use oxide_agent_web_contracts::AgentEffort;

pub(super) const WEB_AGENT_EFFORT: AgentEffort = AgentEffort::Heavy;

pub use workspace::TaskConsole;
