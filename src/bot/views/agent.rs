//! Agent mode UI components
//!
//! Contains keyboards, text messages, and formatters for agent mode.

use crate::agent::loop_detection::LoopType;
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

// ─────────────────────────────────────────────────────────────────────────────
// Trait definition
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for agent UI view rendering
///
/// Provides all text messages and formatting for agent mode interactions.
pub trait AgentView {
    /// Welcome message when agent mode is activated
    fn welcome_message() -> &'static str;

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

    /// Wipe confirmation message
    fn wipe_confirmation() -> &'static str;

    /// Format container recreation error
    fn container_error(error: &str) -> String;

    /// Sandbox access error
    fn sandbox_access_error() -> &'static str;
}

// ─────────────────────────────────────────────────────────────────────────────
// Default implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Default Russian-language implementation of `AgentView`
pub struct DefaultAgentView;

impl AgentView for DefaultAgentView {
    fn welcome_message() -> &'static str {
        r#"🤖 <b>Режим Агента активирован</b>

Жду задачу. Отправьте запрос в любом формате:
• 📝 Текст
• 🎤 Голосовое сообщение
• 🖼 Изображение

Я работаю автономно: сам составлю план, выполню код и предоставлю результат."#
    }

    fn task_processing() -> &'static str {
        "⏳ Обработка задачи..."
    }

    fn task_cancelled(cleared_todos: bool) -> &'static str {
        if cleared_todos {
            "❌ Задача отменяется...\n📋 Список задач очищен."
        } else {
            "❌ Задача отменяется..."
        }
    }

    fn memory_cleared() -> &'static str {
        "🗑 Память агента очищена"
    }

    fn exiting_agent() -> &'static str {
        "👋 Вышли из режима агента"
    }

    fn no_active_task() -> &'static str {
        "⚠️ Нет активной задачи для отмены"
    }

    fn task_already_running() -> &'static str {
        "⏳ Задача уже выполняется. Нажмите ❌ Отменить задачу, если нужно прекратить."
    }

    fn session_not_found() -> &'static str {
        "⚠️ Сессия агента не найдена."
    }

    fn clear_blocked_by_task() -> &'static str {
        "⚠️ Очистка контекста невозможна, пока выполняется задача.\nНажмите «Отменить задачу», дождитесь отмены и затем повторите очистку."
    }

    fn container_recreate_blocked_by_task() -> &'static str {
        "⚠️ Пересоздание контейнера невозможно, пока выполняется задача.\nНажмите «Отменить задачу», дождитесь отмены и затем повторите действие."
    }

    fn container_recreated() -> &'static str {
        "✅ Контейнер успешно пересоздан."
    }

    fn operation_cancelled() -> &'static str {
        "Отменено."
    }

    fn select_keyboard_option() -> &'static str {
        "Пожалуйста, выберите вариант на клавиатуре."
    }

    fn ready_to_work() -> &'static str {
        "Готов к работе."
    }

    fn no_saved_task() -> &'static str {
        "⚠️ Нет сохранённой задачи для повтора."
    }

    fn task_reset() -> &'static str {
        "🔄 Задача сброшена."
    }

    fn reset_blocked_by_task() -> &'static str {
        "⚠️ Нельзя сбросить задачу, пока она выполняется."
    }

    fn loop_detected_message(loop_type: LoopType, iteration: usize) -> String {
        format!(
            "🔁 <b>Обнаружена петля в выполнении задачи</b>\nТип: {}\nИтерация: {}\n\nВыберите действие:",
            loop_type_label(loop_type),
            iteration
        )
    }

    fn error_message(error: &str) -> String {
        format!("❌ Ошибка: {error}")
    }

    fn wipe_confirmation() -> &'static str {
        "⚠️ <b>Внимание!</b>\n\nЭто действие удалит текущий контейнер агента и все файлы внутри него. История переписки сохранится.\n\nВы уверены?"
    }

    fn container_error(error: &str) -> String {
        format!("Ошибка при пересоздании: {error}")
    }

    fn sandbox_access_error() -> &'static str {
        "Ошибка доступа к менеджеру песочницы."
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions
// ─────────────────────────────────────────────────────────────────────────────

/// Get human-readable label for loop type
#[must_use]
pub fn loop_type_label(loop_type: LoopType) -> &'static str {
    match loop_type {
        LoopType::ToolCallLoop => "Повторяющиеся вызовы",
        LoopType::ContentLoop => "Повторяющийся текст",
        LoopType::CognitiveLoop => "Застревание",
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
/// use oxide_agent::bot::views::get_agent_keyboard;
/// let keyboard = get_agent_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_agent_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![
        vec![KeyboardButton::new("❌ Отменить задачу")],
        vec![KeyboardButton::new("🗑 Очистить память")],
        vec![KeyboardButton::new("🔄 Пересоздать контейнер")],
        vec![KeyboardButton::new("⬅️ Выйти из режима агента")],
    ])
    .resize_keyboard()
}

/// Get the loop action inline keyboard
#[must_use]
pub fn loop_action_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("Повторить без детекции", LOOP_CALLBACK_RETRY),
            InlineKeyboardButton::callback("Сбросить задачу", LOOP_CALLBACK_RESET),
        ],
        vec![InlineKeyboardButton::callback(
            "Отменить",
            LOOP_CALLBACK_CANCEL,
        )],
    ])
}

/// Get the wipe confirmation keyboard
#[must_use]
pub fn wipe_confirmation_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![vec![
        KeyboardButton::new("✅ Да"),
        KeyboardButton::new("❌ Отмена"),
    ]])
    .resize_keyboard()
}
