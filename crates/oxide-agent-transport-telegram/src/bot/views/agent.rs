//! Agent mode UI components
//!
//! Contains keyboards, text messages, and formatters for agent mode.

use oxide_agent_core::agent::loop_detection::LoopType;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup, ReplyMarkup,
};

use crate::bot::state::ConfirmationType;

// ─────────────────────────────────────────────────────────────────────────────
// Callback constants
// ─────────────────────────────────────────────────────────────────────────────

/// Callback data for retrying without loop detection
pub const LOOP_CALLBACK_RETRY: &str = "retry_no_loop";
/// Callback data for resetting the current task
pub const LOOP_CALLBACK_RESET: &str = "reset_task";
/// Callback data for cancelling the current task
pub const LOOP_CALLBACK_CANCEL: &str = "cancel_task";
/// Callback data for cancelling the current task from topic controls
pub const AGENT_CALLBACK_CANCEL_TASK: &str = "agent:cancel";
/// Callback data for clearing memory from topic controls
pub const AGENT_CALLBACK_CLEAR_MEMORY: &str = "agent:clear";
/// Callback data for recreating the container from topic controls
pub const AGENT_CALLBACK_RECREATE_CONTAINER: &str = "agent:recreate";
/// Callback prefix for attaching a specific topic-scoped agent flow.
pub const AGENT_CALLBACK_ATTACH_PREFIX: &str = "agent:attach:";
/// Callback data for detaching into a fresh topic-scoped agent flow.
pub const AGENT_CALLBACK_DETACH: &str = "agent:detach";
/// Callback data for exiting agent mode from topic controls
pub const AGENT_CALLBACK_EXIT: &str = "agent:exit";
/// Callback data for confirming memory clear from topic controls
pub const AGENT_CALLBACK_CONFIRM_CLEAR_YES: &str = "agent:confirm:clear:yes";
/// Callback data for cancelling memory clear from topic controls
pub const AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL: &str = "agent:confirm:clear:cancel";
/// Callback data for confirming task cancellation from inline controls
pub const AGENT_CALLBACK_CONFIRM_CANCEL_YES: &str = "agent:confirm:cancel:yes";
/// Callback data for aborting task cancellation from inline controls
pub const AGENT_CALLBACK_CONFIRM_CANCEL_NO: &str = "agent:confirm:cancel:no";
/// Callback data for confirming container recreation from topic controls
pub const AGENT_CALLBACK_CONFIRM_RECREATE_YES: &str = "agent:confirm:recreate:yes";
/// Callback data for cancelling container recreation from topic controls
pub const AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL: &str = "agent:confirm:recreate:cancel";
/// Callback prefix for approving a pending SSH action.
pub const AGENT_CALLBACK_SSH_APPROVE_PREFIX: &str = "agent:ssh:approve:";
/// Callback prefix for rejecting a pending SSH action.
pub const AGENT_CALLBACK_SSH_REJECT_PREFIX: &str = "agent:ssh:reject:";

// ─────────────────────────────────────────────────────────────────────────────
// Trait definition
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for agent UI view rendering
///
/// Provides all text messages and formatting for agent mode interactions.
pub trait AgentView {
    /// Welcome message when agent mode is activated
    fn welcome_message(model_name: &str) -> String;

    /// Message shown while task is processing
    fn task_processing() -> &'static str;

    /// Message when task is cancelled
    fn task_cancelled(cleared_todos: bool) -> &'static str;

    /// Message when memory is cleared
    fn memory_cleared() -> &'static str;

    /// Message when exiting agent mode
    fn exiting_agent() -> &'static str;

    /// Message when no active task to cancel
    fn no_active_task() -> &'static str;

    /// Message when task is already running
    fn task_already_running() -> &'static str;

    /// Message asking to confirm task cancellation
    fn task_cancel_confirmation() -> &'static str;

    /// Message when session not found
    fn session_not_found() -> &'static str;

    /// Message when clearing memory while task is running
    fn clear_blocked_by_task() -> &'static str;
    /// Cannot recreate container while a task is running
    fn container_recreate_blocked_by_task() -> &'static str;

    /// Message for container recreated successfully
    fn container_recreated() -> &'static str;

    /// Message when operation is cancelled
    fn operation_cancelled() -> &'static str;

    /// Message asking to select keyboard option
    fn select_keyboard_option() -> &'static str;

    /// Message when ready to work
    fn ready_to_work() -> &'static str;

    /// No saved task for retry
    fn no_saved_task() -> &'static str;

    /// Task reset confirmation
    fn task_reset() -> &'static str;

    /// Cannot reset while running
    fn reset_blocked_by_task() -> &'static str;

    /// Format loop detected message
    fn loop_detected_message(loop_type: LoopType, iteration: usize) -> String;

    /// Format error message
    fn error_message(error: &str) -> String;

    /// Container wipe confirmation message
    fn container_wipe_confirmation() -> &'static str;

    /// Memory clear confirmation message
    fn memory_clear_confirmation() -> &'static str;

    /// Format container recreation error
    fn container_error(error: &str) -> String;

    /// Sandbox access error
    fn sandbox_access_error() -> &'static str;
}

// ─────────────────────────────────────────────────────────────────────────────
// Default implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Default English-language implementation of `AgentView`
pub struct DefaultAgentView;

impl AgentView for DefaultAgentView {
    fn welcome_message(model_name: &str) -> String {
        format!(
            r#"🤖 <b>Agent Mode Activated - {}</b>

Waiting for a task. Send your request in any format:
• 📝 Text
• 🎤 Voice message
• 🖼 Image

I work autonomously: I'll create a plan, execute code, and provide the result."#,
            model_name
        )
    }

    fn task_processing() -> &'static str {
        "⏳ Processing task..."
    }

    fn task_cancelled(cleared_todos: bool) -> &'static str {
        if cleared_todos {
            "❌ Cancelling task...\n📋 Task list cleared."
        } else {
            "❌ Cancelling task..."
        }
    }

    fn memory_cleared() -> &'static str {
        "🗑 Started a fresh agent context. Previous flows are preserved for re-attach."
    }

    fn exiting_agent() -> &'static str {
        "👋 Exited agent mode"
    }

    fn no_active_task() -> &'static str {
        "⚠️ No active task to cancel"
    }

    fn task_already_running() -> &'static str {
        "⏳ Task is already running. Press ❌ Cancel Task to stop it."
    }

    fn task_cancel_confirmation() -> &'static str {
        "⚠️ Cancel the current task?"
    }

    fn session_not_found() -> &'static str {
        "⚠️ Agent session not found."
    }

    fn clear_blocked_by_task() -> &'static str {
        "⚠️ Cannot clear context while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    fn container_recreate_blocked_by_task() -> &'static str {
        "⚠️ Cannot recreate container while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    fn container_recreated() -> &'static str {
        "✅ Container successfully recreated."
    }

    fn operation_cancelled() -> &'static str {
        "Cancelled."
    }

    fn select_keyboard_option() -> &'static str {
        "Please select an option on the keyboard."
    }

    fn ready_to_work() -> &'static str {
        "Ready to work."
    }

    fn no_saved_task() -> &'static str {
        "⚠️ No saved task to retry."
    }

    fn task_reset() -> &'static str {
        "🔄 Task reset."
    }

    fn reset_blocked_by_task() -> &'static str {
        "⚠️ Cannot reset task while it is running."
    }

    fn loop_detected_message(loop_type: LoopType, iteration: usize) -> String {
        format!(
            "🔁 <b>Loop detected in task execution</b>\nType: {}\nIteration: {}\n\nChoose an action:",
            loop_type_label(loop_type),
            iteration
        )
    }

    fn error_message(error: &str) -> String {
        format!("❌ Error: {error}")
    }

    fn container_wipe_confirmation() -> &'static str {
        "⚠️ <b>Warning!</b>\n\nThis action will delete the current agent container and all files inside it. Chat history will be preserved.\n\nAre you sure?"
    }

    fn memory_clear_confirmation() -> &'static str {
        "⚠️ <b>Warning!</b>\n\nThis action will start a fresh agent flow for this topic. Previous flows will be preserved and can be attached later. The container and files will remain intact.\n\nAre you sure?"
    }

    fn container_error(error: &str) -> String {
        format!("Error during recreation: {error}")
    }

    fn sandbox_access_error() -> &'static str {
        "Sandbox manager access error."
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions
// ─────────────────────────────────────────────────────────────────────────────

/// Get human-readable label for loop type
#[must_use]
pub fn loop_type_label(loop_type: LoopType) -> &'static str {
    match loop_type {
        LoopType::ToolCallLoop => "Repetitive calls",
        LoopType::ContentLoop => "Repetitive text",
        LoopType::CognitiveLoop => "Stuck",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Keyboards
// ─────────────────────────────────────────────────────────────────────────────

/// Get the agent mode keyboard
///
/// # Examples
///
/// ```
/// use oxide_agent_transport_telegram::bot::views::get_agent_keyboard;
/// let keyboard = get_agent_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_agent_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![
        vec![KeyboardButton::new("❌ Cancel Task")],
        vec![KeyboardButton::new("🗑 Clear Memory")],
        vec![KeyboardButton::new("🔄 Recreate Container")],
        vec![KeyboardButton::new("⬅️ Exit Agent Mode")],
    ])
    .resize_keyboard()
}

/// Get topic-friendly inline controls for agent mode.
#[must_use]
pub fn get_agent_inline_keyboard(agent_flow_id: Option<&str>) -> InlineKeyboardMarkup {
    get_agent_inline_keyboard_with_exit(true, agent_flow_id)
}

/// Get topic-friendly inline controls for agent mode with optional exit action.
#[must_use]
pub fn get_agent_inline_keyboard_with_exit(
    include_exit: bool,
    agent_flow_id: Option<&str>,
) -> InlineKeyboardMarkup {
    let mut keyboard = vec![
        vec![InlineKeyboardButton::callback(
            "❌ Cancel Task",
            AGENT_CALLBACK_CANCEL_TASK,
        )],
        vec![InlineKeyboardButton::callback(
            "🗑 Clear Memory",
            AGENT_CALLBACK_CLEAR_MEMORY,
        )],
        vec![InlineKeyboardButton::callback(
            "🔄 Recreate Container",
            AGENT_CALLBACK_RECREATE_CONTAINER,
        )],
    ];
    if let Some(agent_flow_id) = agent_flow_id {
        keyboard.push(vec![
            InlineKeyboardButton::callback(
                "🔗 Attach",
                format!("{AGENT_CALLBACK_ATTACH_PREFIX}{agent_flow_id}"),
            ),
            InlineKeyboardButton::callback("✂️ Detach", AGENT_CALLBACK_DETACH),
        ]);
    }
    if include_exit {
        keyboard.push(vec![InlineKeyboardButton::callback(
            "⬅️ Exit Agent Mode",
            AGENT_CALLBACK_EXIT,
        )]);
    }

    InlineKeyboardMarkup::new(keyboard)
}

/// Get inline controls for an active progress message in topics.
#[must_use]
pub fn progress_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "❌ Cancel Task",
        AGENT_CALLBACK_CANCEL_TASK,
    )]])
}

/// Get an empty inline keyboard to clear topic controls.
#[must_use]
pub fn empty_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new())
}

/// Get inline confirmation controls for task cancellation.
#[must_use]
pub fn cancel_task_confirmation_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("Yes", AGENT_CALLBACK_CONFIRM_CANCEL_YES),
        InlineKeyboardButton::callback("No", AGENT_CALLBACK_CONFIRM_CANCEL_NO),
    ]])
}

/// Get agent controls markup for the current chat context.
#[must_use]
pub fn agent_control_markup(use_inline: bool) -> ReplyMarkup {
    if use_inline {
        get_agent_inline_keyboard(None).into()
    } else {
        get_agent_keyboard().into()
    }
}

/// Get inline flow controls for the final agent response in topics.
#[must_use]
pub fn agent_flow_inline_keyboard(agent_flow_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            "🔗 Attach",
            format!("{AGENT_CALLBACK_ATTACH_PREFIX}{agent_flow_id}"),
        ),
        InlineKeyboardButton::callback("✂️ Detach", AGENT_CALLBACK_DETACH),
    ]])
}

/// Get the loop action inline keyboard
#[must_use]
pub fn loop_action_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("Retry w/o detection", LOOP_CALLBACK_RETRY),
            InlineKeyboardButton::callback("Reset task", LOOP_CALLBACK_RESET),
        ],
        vec![InlineKeyboardButton::callback(
            "Cancel",
            LOOP_CALLBACK_CANCEL,
        )],
    ])
}

/// Get the confirmation keyboard for destructive actions
#[must_use]
pub fn confirmation_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![vec![
        KeyboardButton::new("✅ Yes"),
        KeyboardButton::new("❌ Cancel"),
    ]])
    .resize_keyboard()
}

/// Get topic-friendly confirmation controls.
#[must_use]
pub fn confirmation_inline_keyboard(action: ConfirmationType) -> InlineKeyboardMarkup {
    let (yes_callback, cancel_callback) = match action {
        ConfirmationType::ClearMemory => (
            AGENT_CALLBACK_CONFIRM_CLEAR_YES,
            AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL,
        ),
        ConfirmationType::RecreateContainer => (
            AGENT_CALLBACK_CONFIRM_RECREATE_YES,
            AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL,
        ),
    };

    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("Yes", yes_callback),
        InlineKeyboardButton::callback("Cancel", cancel_callback),
    ]])
}

/// Get inline approval controls for a pending SSH action.
#[must_use]
pub fn ssh_approval_inline_keyboard(request_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            "Approve",
            format!("{AGENT_CALLBACK_SSH_APPROVE_PREFIX}{request_id}"),
        ),
        InlineKeyboardButton::callback(
            "Reject",
            format!("{AGENT_CALLBACK_SSH_REJECT_PREFIX}{request_id}"),
        ),
    ]])
}

/// Get confirmation markup for the current chat context.
#[must_use]
pub fn confirmation_markup(use_inline: bool, action: ConfirmationType) -> ReplyMarkup {
    if use_inline {
        confirmation_inline_keyboard(action).into()
    } else {
        confirmation_keyboard().into()
    }
}
