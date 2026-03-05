// implementation/session.rs
//
// Agent Session Management
//
// Этот файл управляет сессиями агента, включая health checks и cancellation.

use crate::agent::registry::ToolRegistry;
use tokio_util::sync::CancellationToken;

/// Agent Session
///
/// Управляет сессией агента, включая tool registry и cancellation token.
pub struct AgentSession {
    /// Уникальный ID сессии
    pub id: String,

    /// ID пользователя
    pub user_id: String,

    /// Tool registry для маршрутизации инструментов
    pub registry: ToolRegistry,

    /// Cancellation token для прерывания выполнения
    pub cancellation_token: CancellationToken,

    /// Статус сессии
    pub status: SessionStatus,
}

/// Статус сессии
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    /// Сессия активна
    Active,
    /// Выполнение задачи
    Running,
    /// Ошибка выполнения
    Error,
    /// Сессия завершена
    Completed,
    /// Сессия отменена
    Cancelled,
}

impl AgentSession {
    /// Создать новую сессию
    ///
    /// # Arguments
    ///
    /// * `user_id` - ID пользователя
    /// * `opencode_url` - URL Opencode Server (опционально)
    ///
    /// # Example
    ///
    /// ```
    /// let session = AgentSession::new(
    ///     "user-123".to_string(),
    ///     Some("http://127.0.0.1:4096".to_string())
    /// );
    /// ```
    pub fn new(user_id: String, opencode_url: Option<String>) -> Self {
        let registry = ToolRegistry::new(opencode_url);
        let cancellation_token = CancellationToken::new();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id,
            registry,
            cancellation_token,
            status: SessionStatus::Active,
        }
    }

    /// Проверить здоровье Opencode сервера
    ///
    /// # Returns
    ///
    /// `Ok(())` если сервер здоров, `Err` с ошибкой иначе
    pub async fn check_opencode_health(&self) -> Result<(), String> {
        self.registry.health_check().await
    }

    /// Отменить выполнение задачи
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
        self.status = SessionStatus::Cancelled;
    }

    /// Получить состояние cancellation
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Обновить статус сессии
    pub fn update_status(&mut self, status: SessionStatus) {
        self.status = status;
    }

    /// Создать child cancellation token
    pub fn child_token(&self) -> CancellationToken {
        self.cancellation_token.child_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = AgentSession::new(
            "user-123".to_string(),
            Some("http://127.0.0.1:4096".to_string()),
        );

        assert_eq!(session.user_id, "user-123");
        assert_eq!(session.status, SessionStatus::Active);
        assert!(!session.id.is_empty());
    }

    #[test]
    fn test_session_cancellation() {
        let session = AgentSession::new(
            "user-456".to_string(),
            None,
        );

        assert!(!session.is_cancelled());

        session.cancel();
        assert!(session.is_cancelled());
        assert_eq!(session.status, SessionStatus::Cancelled);
    }

    #[test]
    fn test_session_status_update() {
        let mut session = AgentSession::new(
            "user-789".to_string(),
            None,
        );

        assert_eq!(session.status, SessionStatus::Active);

        session.update_status(SessionStatus::Running);
        assert_eq!(session.status, SessionStatus::Running);

        session.update_status(SessionStatus::Completed);
        assert_eq!(session.status, SessionStatus::Completed);
    }

    #[test]
    fn test_child_token() {
        let session = AgentSession::new(
            "user-abc".to_string(),
            None,
        );

        let child_token = session.child_token();
        assert!(!child_token.is_cancelled());

        session.cancel();
        assert!(session.is_cancelled());
        assert!(child_token.is_cancelled());
    }
}
