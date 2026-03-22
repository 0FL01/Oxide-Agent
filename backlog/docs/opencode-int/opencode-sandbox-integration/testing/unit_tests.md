# Unit Tests

> **Unit тесты для компонентов интеграции**
>
> 📁 **Раздел:** Testing
> 🎯 **Цель:** Понять как тестировать компоненты

---

## 📋 Оглавление

- [OpencodeToolProvider тесты](#opencodetoolprovider-тесты)
- [ToolRegistry тесты](#toolregistry-тесты)
- [AgentSession тесты](#agentsession-тесты)
- [SandboxProvider тесты](#sandboxprovider-тесты)
- [Mocking](#mocking)

---

## OpencodeToolProvider тесты

### Тест 1: Health check

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_health_check() {
        let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

        let result = provider.health_check().await;

        assert!(result.is_ok(), "Health check should succeed");
    }
}
```

### Тест 2: Create session

```rust
#[tokio::test]
#[ignore = "Requires running Opencode server"]
async fn test_create_session() {
    let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

    let result = provider.create_session("test task").await;

    assert!(result.is_ok(), "Session creation should succeed");

    let session = result.unwrap();
    assert!(!session.id.is_empty(), "Session ID should not be empty");
    assert!(session.title.contains("test"), "Title should contain task");
}
```

### Тест 3: Send prompt

```rust
#[tokio::test]
#[ignore = "Requires running Opencode server"]
async fn test_send_prompt() {
    let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

    // First create a session
    let session = provider.create_session("test").await.unwrap();

    // Send a prompt
    let result = provider.send_prompt(&session.id, "say hello").await;

    assert!(result.is_ok(), "Prompt sending should succeed");

    let response = result.unwrap();
    assert!(!response.parts.is_empty(), "Response should have parts");
}
```

### Тест 4: Execute task

```rust
#[tokio::test]
#[ignore = "Requires running Opencode server"]
async fn test_execute_task() {
    let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

    let result = provider.execute_task("list files in current directory").await;

    assert!(result.is_ok(), "Task execution should succeed");

    let output = result.unwrap();
    assert!(!output.is_empty(), "Output should not be empty");
}
```

---

## ToolRegistry тесты

### Тест 1: Execute opencode tool

```rust
#[tokio::test]
#[ignore = "Requires running Opencode server"]
async fn test_execute_opencode_tool() {
    let registry = ToolRegistry::new(Some("http://127.0.0.1:4096".to_string()));
    let (progress_tx, mut progress_rx) = mpsc::channel(100);

    let args = r#"{"task": "list files"}"#;
    let result = registry.execute("opencode", args, &progress_tx, None).await;

    assert!(result.is_ok(), "Opencode tool should execute");

    // Check progress events
    let event = progress_rx.recv().await.unwrap();
    matches!(event, AgentEvent::ToolCall { name, .. } if name == "opencode");

    let event = progress_rx.recv().await.unwrap();
    matches!(event, AgentEvent::ToolResult { name, .. } if name == "opencode");
}
```

### Тест 2: Unknown tool

```rust
#[tokio::test]
async fn test_unknown_tool() {
    let registry = ToolRegistry::new(None);
    let (progress_tx, _progress_rx) = mpsc::channel(100);

    let result = registry.execute("unknown_tool", "{}", &progress_tx, None).await;

    assert!(result.is_err(), "Unknown tool should return error");
    assert!(result.unwrap_err().contains("Tool not found"));
}
```

### Тест 3: Health check

```rust
#[tokio::test]
#[ignore = "Requires running Opencode server"]
async fn test_health_check() {
    let registry = ToolRegistry::new(Some("http://127.0.0.1:4096".to_string()));

    let result = registry.health_check().await;

    assert!(result.is_ok(), "Health check should succeed");
}
```

---

## AgentSession тесты

### Тест 1: Session creation

```rust
#[test]
fn test_session_creation() {
    let session = AgentSession::new(
        "user-123".to_string(),
        Some("http://127.0.0.1:4096".to_string())
    );

    assert_eq!(session.user_id, "user-123");
    assert_eq!(session.status, SessionStatus::Active);
    assert!(!session.id.is_empty());
    assert!(!session.is_cancelled());
}
```

### Тест 2: Session cancellation

```rust
#[test]
fn test_session_cancellation() {
    let session = AgentSession::new(
        "user-456".to_string(),
        None
    );

    assert!(!session.is_cancelled());

    session.cancel();
    assert!(session.is_cancelled());
    assert_eq!(session.status, SessionStatus::Cancelled);
}
```

### Тест 3: Child token

```rust
#[test]
fn test_child_token() {
    let session = AgentSession::new(
        "user-789".to_string(),
        None
    );

    let child_token = session.child_token();
    assert!(!child_token.is_cancelled());

    session.cancel();
    assert!(session.is_cancelled());
    assert!(child_token.is_cancelled(), "Child token should also be cancelled");
}
```

---

## SandboxProvider тесты

### Тест 1: Execute command

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_execute_command() {
    let mut sandbox = SandboxProvider::new("agent-sandbox:latest".to_string()).await;

    let result = sandbox.execute_command("echo 'Hello, World!'").await;

    assert!(result.is_ok(), "Command execution should succeed");

    let output = result.unwrap();
    assert!(output.contains("Hello, World!"));
}
```

### Тест 2: Write and read file

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_write_and_read_file() {
    let mut sandbox = SandboxProvider::new("agent-sandbox:latest".to_string()).await;

    // Write file
    let content = b"Hello, File!";
    let write_result = sandbox.write_file("/workspace/test.txt", content).await;
    assert!(write_result.is_ok(), "File writing should succeed");

    // Read file
    let read_result = sandbox.read_file("/workspace/test.txt").await;
    assert!(read_result.is_ok(), "File reading should succeed");

    let read_content = read_result.unwrap();
    assert_eq!(read_content, content);
}
```

### Тест 3: List files

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_list_files() {
    let mut sandbox = SandboxProvider::new("agent-sandbox:latest".to_string()).await;

    // Create test files
    sandbox.write_file("/workspace/file1.txt", b"content1").await.unwrap();
    sandbox.write_file("/workspace/file2.txt", b"content2").await.unwrap();

    // List files
    let result = sandbox.list_files("/workspace").await;

    assert!(result.is_ok(), "File listing should succeed");

    let files = result.unwrap();
    assert!(files.len() >= 2);
    assert!(files.iter().any(|f| f.contains("file1.txt")));
    assert!(files.iter().any(|f| f.contains("file2.txt")));
}
```

---

## Mocking

### Mock HTTP клиент для OpencodeToolProvider

```rust
use reqwest::Client;
use mockito::{mock, Server};

#[tokio::test]
async fn test_opencode_provider_with_mock() {
    let mut server = Server::new();

    // Mock health check
    let health_mock = mock("GET", "/vcs")
        .with_status(200)
        .with_body(r#"{"branch":"main"}"#)
        .create();

    // Mock session creation
    let session_mock = mock("POST", "/session")
        .with_status(200)
        .with_body(r#"{"id":"session-123","title":"Task"}"#)
        .create();

    // Use mock server URL
    let provider = OpencodeToolProvider::new(server.url());

    // Test health check
    let health_result = provider.health_check().await;
    assert!(health_result.is_ok());
    health_mock.assert();

    // Test session creation
    let session_result = provider.create_session("test task").await;
    assert!(session_result.is_ok());
    session_mock.assert();
}
```

### Mock SandboxProvider

```rust
use mockall::mock;

#[async_trait]
impl SandboxProvider for MockSandboxProvider {
    async fn execute_command(&mut self, cmd: &str) -> Result<String, Error> {
        if cmd.contains("error") {
            return Err(Error::CommandFailed("Mock error".to_string()));
        }
        Ok("Mock output".to_string())
    }

    async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<(), Error> {
        Ok(())
    }

    async fn read_file(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        Ok(b"Mock content".to_vec())
    }

    async fn list_files(&mut self, path: &str) -> Result<Vec<String>, Error> {
        Ok(vec!["file1.txt".to_string(), "file2.txt".to_string()])
    }
}
```

### Mock ToolRegistry

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_registry_with_mock() {
        let mut mock_opencode = MockOpencodeProvider::new();
        let mock_sandbox = MockSandboxProvider::new();

        // Setup expectations
        mock_opencode
            .expect_execute_task()
            .with(mockall::predicate::eq("test task"))
            .returning(Ok("Mock result".to_string()));

        // Create registry with mocks
        let registry = ToolRegistry::with_providers(
            vec![Box::new(mock_sandbox)],
            mock_opencode,
        );

        // Execute tool
        let result = registry.execute("opencode", r#"{"task":"test task"}"#, &tx, None).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Mock result");
    }
}
```

---

## Запуск тестов

### Все тесты

```bash
# Запустить все тесты
cargo test

# С ignored (нужен Opencode server)
cargo test -- --ignored

# С выводом
cargo test -- --nocapture

# Параллельно
cargo test -- --test-threads=4
```

### Конкретный тест

```bash
# Запустить конкретный тест
cargo test test_health_check

# Запустить все тесты в модуле
cargo test opencode_provider::tests

# Запустить тесты в файле
cargo test integration_examples
```

### Фильтрация

```bash
# Запустить тесты с фильтром
cargo test opencode

# Запустить тесты без фильтра
cargo test -- '' opencode

# Запустить точное совпадение
cargo test --exact test_health_check
```

---

## Покрытие тестов

### Добавить cargo-tarpaulin

```bash
# В dev dependencies
cargo install cargo-tarpaulin
```

### Запустить с покрытием

```bash
# Линейное покрытие
cargo tarpaulin --out Html

# HTML отчет
cargo tarpaulin --out Html --output-dir coverage/

# Консольный отчет
cargo tarpaulin --stdout
```

### Цели покрытия

| Тип           | Минимум | Рекомендуется |
| ------------- | ------- | ------------- |
| **Lines**     | 70%     | 90%           |
| **Functions** | 70%     | 90%           |
| **Branches**  | 60%     | 80%           |

---

## Следующие шаги

- [ ] Изучить [integration_tests.md](./integration_tests.md) - Интеграционные тесты
- [ ] Изучить [troubleshooting.md](./troubleshooting.md) - Устранение проблем
- [ ] Перейти к [deployment/](../deployment/) - Деплой

---

**Связанные документы:**

- [testing/integration_tests.md](./integration_tests.md) - Интеграционные тесты
- [testing/troubleshooting.md](./troubleshooting.md) - Устранение проблем
- [implementation/](../implementation/) - Код реализации
