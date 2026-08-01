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
/// Callback data for opening the Agent model selector.
pub const AGENT_CALLBACK_MODEL_OPEN: &str = "m:o";
/// Callback prefix for attaching a specific topic-scoped agent flow.
pub const AGENT_CALLBACK_ATTACH_PREFIX: &str = "agent:attach:";
/// Callback data for detaching into a fresh topic-scoped agent flow.
pub const AGENT_CALLBACK_DETACH: &str = "agent:detach";
/// Callback data for confirming memory clear from topic controls
pub const AGENT_CALLBACK_CONFIRM_CLEAR_YES: &str = "agent:confirm:clear:yes";
/// Callback data for cancelling memory clear from topic controls
pub const AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL: &str = "agent:confirm:clear:cancel";
/// Callback data for confirming context compaction from topic controls
pub const AGENT_CALLBACK_CONFIRM_COMPACT_YES: &str = "agent:confirm:compact:yes";
/// Callback data for cancelling context compaction from topic controls
pub const AGENT_CALLBACK_CONFIRM_COMPACT_CANCEL: &str = "agent:confirm:compact:cancel";
/// Callback data for confirming task cancellation from inline controls
pub const AGENT_CALLBACK_CONFIRM_CANCEL_YES: &str = "agent:confirm:cancel:yes";
/// Callback data for aborting task cancellation from inline controls
pub const AGENT_CALLBACK_CONFIRM_CANCEL_NO: &str = "agent:confirm:cancel:no";
/// Callback data for confirming container recreation from topic controls
pub const AGENT_CALLBACK_CONFIRM_RECREATE_YES: &str = "agent:confirm:recreate:yes";
/// Callback data for cancelling container recreation from topic controls
pub const AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL: &str = "agent:confirm:recreate:cancel";
pub(crate) struct DefaultAgentView;

impl DefaultAgentView {
    pub(crate) fn welcome_message(model_name: &str) -> String {
        let model_name = html_escape::encode_text(model_name);
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

    pub(crate) fn task_processing() -> &'static str {
        "⏳ Processing task..."
    }

    pub(crate) fn task_cancelling() -> &'static str {
        "❌ Cancelling task..."
    }

    pub(crate) fn task_cancelled() -> &'static str {
        "❌ Task canceled"
    }

    pub(crate) fn memory_cleared() -> &'static str {
        "🗑 Started a fresh agent context. Previous flows are preserved for re-attach."
    }

    pub(crate) fn no_active_task() -> &'static str {
        "⚠️ No active task to cancel"
    }

    pub(crate) fn task_already_running() -> &'static str {
        "⏳ Task is already running. Press ❌ Cancel Task to stop it."
    }

    pub(crate) fn task_cancel_confirmation() -> &'static str {
        "⚠️ Cancel the current task?"
    }

    pub(crate) fn session_not_found() -> &'static str {
        "⚠️ Agent session not found."
    }

    pub(crate) fn clear_blocked_by_task() -> &'static str {
        "⚠️ Cannot clear context while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    pub(crate) fn compact_blocked_by_task() -> &'static str {
        "⚠️ Cannot compact context while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    pub(crate) fn container_recreate_blocked_by_task() -> &'static str {
        "⚠️ Cannot recreate container while a task is running.\nPress \"Cancel Task\", wait for cancellation, then try again."
    }

    pub(crate) fn context_compacting() -> &'static str {
        "🗜 Compacting agent context..."
    }

    pub(crate) fn context_compacted(applied: bool) -> &'static str {
        if applied {
            "🗜 Agent context compacted. You can continue the same flow."
        } else {
            "🗜 Agent context is already compact enough."
        }
    }

    pub(crate) fn container_recreated() -> &'static str {
        "✅ Container successfully recreated."
    }

    pub(crate) fn operation_cancelled() -> &'static str {
        "Cancelled."
    }

    pub(crate) fn select_keyboard_option() -> &'static str {
        "Please select an option on the keyboard."
    }

    pub(crate) fn ready_to_work() -> &'static str {
        "Ready to work."
    }

    pub(crate) fn no_saved_task() -> &'static str {
        "⚠️ No saved task to retry."
    }

    pub(crate) fn task_reset() -> &'static str {
        "🔄 Task reset."
    }

    pub(crate) fn reset_blocked_by_task() -> &'static str {
        "⚠️ Cannot reset task while it is running."
    }

    pub(crate) fn error_message(error: &str) -> String {
        format!("❌ Error: {error}")
    }

    pub(crate) fn container_wipe_confirmation() -> &'static str {
        "⚠️ <b>Warning!</b>\n\nThis action will delete the current agent container and all files inside it. Chat history will be preserved.\n\nAre you sure?"
    }

    pub(crate) fn memory_clear_confirmation() -> &'static str {
        "⚠️ <b>Warning!</b>\n\nThis action will start a fresh agent flow for this topic. Previous flows will be preserved and can be attached later. The container and files will remain intact.\n\nAre you sure?"
    }

    pub(crate) fn context_compaction_confirmation() -> &'static str {
        "⚠️ <b>Warning!</b>\n\nThis action will compact the current agent context for this flow. The agent may summarize older working history, but the current flow will remain active.\n\nAre you sure?"
    }

    pub(crate) fn container_error(error: &str) -> String {
        format!("Error during recreation: {error}")
    }

    pub(crate) fn sandbox_access_error() -> &'static str {
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
    KeyboardMarkup::new(vec![vec![
        KeyboardButton::new("❌ Cancel Task"),
        KeyboardButton::new("🧠 Model"),
    ]])
    .resize_keyboard()
}

/// Get topic-friendly inline controls for agent mode.
#[must_use]
pub fn get_agent_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("❌ Cancel Task", AGENT_CALLBACK_CANCEL_TASK),
        InlineKeyboardButton::callback("🧠 Model", AGENT_CALLBACK_MODEL_OPEN),
    ]])
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
        get_agent_inline_keyboard().into()
    } else {
        get_agent_keyboard().into()
    }
}

/// Get inline flow controls for the final agent response in topics.
#[must_use]
pub fn agent_flow_inline_keyboard_with_toggle(
    agent_flow_id: &str,
    attach_detach_enabled: bool,
) -> InlineKeyboardMarkup {
    agent_flow_inline_keyboard_with_options(agent_flow_id, attach_detach_enabled, false)
}

/// Get inline flow controls with an optional Agent model button.
#[must_use]
pub fn agent_flow_inline_keyboard_with_options(
    agent_flow_id: &str,
    attach_detach_enabled: bool,
    include_model: bool,
) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    if attach_detach_enabled {
        rows.push(vec![
            InlineKeyboardButton::callback(
                "🔗 Attach",
                format!("{AGENT_CALLBACK_ATTACH_PREFIX}{agent_flow_id}"),
            ),
            InlineKeyboardButton::callback("✂️ Detach", AGENT_CALLBACK_DETACH),
        ]);
    }
    if include_model {
        rows.push(vec![InlineKeyboardButton::callback(
            "🧠 Model",
            AGENT_CALLBACK_MODEL_OPEN,
        )]);
    }
    InlineKeyboardMarkup::new(rows)
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
        ConfirmationType::CompactContext => (
            AGENT_CALLBACK_CONFIRM_COMPACT_YES,
            AGENT_CALLBACK_CONFIRM_COMPACT_CANCEL,
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

/// Get confirmation markup for the current chat context.
#[must_use]
pub fn confirmation_markup(use_inline: bool, action: ConfirmationType) -> ReplyMarkup {
    if use_inline {
        confirmation_inline_keyboard(action).into()
    } else {
        confirmation_keyboard().into()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DefaultAgentView, agent_flow_inline_keyboard_with_toggle, get_agent_inline_keyboard,
        get_agent_keyboard,
    };

    #[test]
    fn cancellation_messages_use_distinct_in_progress_and_terminal_text() {
        assert_eq!(DefaultAgentView::task_cancelling(), "❌ Cancelling task...");
        assert_eq!(DefaultAgentView::task_cancelled(), "❌ Task canceled");
    }

    #[test]
    fn agent_control_keyboards_include_cancel_and_model() {
        let keyboard = get_agent_keyboard();
        let buttons: Vec<_> = keyboard.keyboard.iter().flatten().collect();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].text, "❌ Cancel Task");
        assert_eq!(buttons[1].text, "🧠 Model");
        assert_no_browser_control_text(&buttons[0].text);

        let inline = get_agent_inline_keyboard();
        let inline_buttons: Vec<_> = inline.inline_keyboard.iter().flatten().collect();
        assert_eq!(inline_buttons.len(), 2);
        assert_eq!(inline_buttons[0].text, "❌ Cancel Task");
        assert_eq!(inline_buttons[1].text, "🧠 Model");
        assert_no_browser_control_text(&inline_buttons[0].text);
        assert_no_browser_control_text(&format!("{:?}", inline_buttons[0].kind));
    }

    fn assert_no_browser_control_text(value: &str) {
        let value = value.to_ascii_lowercase();
        assert!(!value.contains("browser"));
        assert!(!value.contains("chrome"));
        assert!(!value.contains("start"));
        assert!(!value.contains("control"));
    }

    #[test]
    fn inline_keyboards_hide_attach_detach_when_disabled() {
        let flow_controls = agent_flow_inline_keyboard_with_toggle("flow-1", false);
        assert!(flow_controls.inline_keyboard.is_empty());
    }
}
