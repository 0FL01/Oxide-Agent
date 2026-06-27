//! Prompt module
//!
//! Contains prompt composition logic for the agent.

pub mod composer;
pub mod context;

pub use composer::{
    ComposedPrompt, PromptToolContext, create_agent_system_prompt, create_sub_agent_system_prompt,
};
pub use context::{
    DynamicPromptContextProvider, PromptContextBlock, PromptContextError, PromptContextRequest,
    PromptContextResult, PromptContextSemantics,
};
