//! Agent mode UI components
//!
//! Contains keyboards, text messages, and formatters for agent mode.

use oxide_agent_core::agent::loop_detection::LoopType;
use oxide_agent_core::agent::TaskId;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup};

// ─────────────────────────────────────────────────────────────────────────────
// Callback constants
// ─────────────────────────────────────────────────────────────────────────────

/// Callback data for retrying without loop detection
pub const LOOP_CALLBACK_RETRY: &str = "retry_no_loop";
/// Callback data for resetting the current task
pub const LOOP_CALLBACK_RESET: &str = "reset_task";
/// Callback data for cancelling the current task
pub const LOOP_CALLBACK_CANCEL: &str = "cancel_task";
/// Prefix for task control callbacks.
pub const TASK_CONTROL_CALLBACK_PREFIX: &str = "task_control";
/// Callback action for runtime cancellation.
pub const TASK_CONTROL_ACTION_CANCEL: &str = "cancel";
/// Callback action for graceful stop with report.
pub const TASK_CONTROL_ACTION_STOP: &str = "stop";

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

    /// Message when graceful stop is requested
    fn task_stop_requested() -> &'static str;

    /// Message when memory is cleared
    fn memory_cleared() -> &'static str;

    /// Message when exiting agent mode
    fn exiting_agent() -> &'static str;

    /// Message when exit is blocked by an active task
    fn exit_blocked_by_task() -> &'static str;

    /// Message when no active task to cancel
    fn no_active_task() -> &'static str;

    /// Message when no active task to stop
    fn no_active_task_to_stop() -> &'static str;

    /// Message when task is already running
    fn task_already_running() -> &'static str;

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

    fn task_stop_requested() -> &'static str {
        "🛑 Stop requested. The task will stop at the next safe point and send a report."
    }

    fn memory_cleared() -> &'static str {
        "🗑 Agent memory cleared"
    }

    fn exiting_agent() -> &'static str {
        "👋 Exited agent mode"
    }

    fn exit_blocked_by_task() -> &'static str {
        "⚠️ Cannot exit agent mode while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    fn no_active_task() -> &'static str {
        "⚠️ No active task to cancel"
    }

    fn no_active_task_to_stop() -> &'static str {
        "⚠️ No active task to stop"
    }

    fn task_already_running() -> &'static str {
        "⏳ Task is already running. Press ❌ Cancel Task to stop it."
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
        "⚠️ <b>Warning!</b>\n\nThis action will clear the agent's entire conversation history. The container and files will remain intact.\n\nAre you sure?"
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
        vec![
            KeyboardButton::new("❌ Cancel Task"),
            KeyboardButton::new("🛑 Stop with Report"),
        ],
        vec![KeyboardButton::new("🗑 Clear Memory")],
        vec![KeyboardButton::new("🔄 Recreate Container")],
        vec![KeyboardButton::new("⬅️ Exit Agent Mode")],
    ])
    .resize_keyboard()
}

/// Get inline task control keyboard bound to a task.
#[must_use]
pub fn task_control_keyboard(task_id: TaskId) -> InlineKeyboardMarkup {
    let cancel = format!("{TASK_CONTROL_CALLBACK_PREFIX}:{TASK_CONTROL_ACTION_CANCEL}:{task_id}");
    let stop = format!("{TASK_CONTROL_CALLBACK_PREFIX}:{TASK_CONTROL_ACTION_STOP}:{task_id}");

    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("❌ Cancel", cancel),
        InlineKeyboardButton::callback("🛑 Stop", stop),
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
