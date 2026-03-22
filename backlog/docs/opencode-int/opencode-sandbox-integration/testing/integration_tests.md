# Integration Tests

> **Интеграционные тесты для всей системы**
>
> 📁 **Раздел:** Testing
> 🎯 **Цель:** Понять как тестировать интеграцию

---

## 📋 Оглавление

- [Настройка окружения](#настройка-окружения)
- [Opencode интеграционные тесты](#opencode-интеграционные-тесты)
- [Sandbox интеграционные тесты](#sandbox-интеграционные-тесты)
- [End-to-end тесты](#end-to-end-тесты)
- [Performance тесты](#performance-тесты)

---

## Настройка окружения

### Testcontainers для Docker

Добавить в `Cargo.toml`:

```toml
[dev-dependencies]
testcontainers = "0.14"
```

### Запуск Opencode server

```rust
use testcontainers::clients::Cli;
use std::process::Command;

pub async fn start_opencode_server() -> String {
    let docker = Cli::default();

    // Запустить Opencode контейнер
    let container = docker.run(
        testcontainers::images::generic::GenericImage::new("opencode", "latest")
    ).await;

    let port = container.get_host_port_ipv4(4096).await;
    let url = format!("http://127.0.0.1:{}", port);

    // Дождаться readiness
    tokio::time::sleep(Duration::from_secs(5)).await;

    url
}
```

### Test setup

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Общий setup для всех тестов
    async fn setup() -> TestContext {
        let opencode_url = start_opencode_server().await;
        let registry = ToolRegistry::new(Some(opencode_url.clone()));
        let (progress_tx, progress_rx) = mpsc::channel(100);

        TestContext {
            opencode_url,
            registry,
            progress_tx,
            progress_rx,
        }
    }

    struct TestContext {
        pub opencode_url: String,
        pub registry: ToolRegistry,
        pub progress_tx: mpsc::Sender<AgentEvent>,
        pub progress_rx: mpsc::Receiver<AgentEvent>,
    }

    async fn teardown(ctx: TestContext) {
        // Очистка после теста
        drop(ctx);
    }
}
```

---

## Opencode интеграционные тесты

### Тест 1: Полный workflow с Opencode

```rust
#[tokio::test]
#[ignore = "Requires Opencode server"]
async fn test_full_opencode_workflow() {
    let ctx = setup().await;

    // Создать session
    let args = r#"{"task": "list files in current directory"}"#;
    let result = ctx.registry.execute("opencode", args, &ctx.progress_tx, None).await;

    assert!(result.is_ok(), "Task should execute successfully");

    // Проверить прогресс события
    let tool_call = ctx.progress_rx.recv().await.unwrap();
    matches!(tool_call, AgentEvent::ToolCall { name, .. } if name == "opencode");

    let tool_result = ctx.progress_rx.recv().await.unwrap();
    matches!(tool_result, AgentEvent::ToolResult { name, .. } if name == "opencode");

    teardown(ctx).await;
}
```

### Тест 2: Несколько задач подряд

```rust
#[tokio::test]
#[ignore = "Requires Opencode server"]
async fn test_multiple_opencode_tasks() {
    let ctx = setup().await;

    // Task 1
    let result1 = ctx.registry.execute(
        "opencode",
        r#"{"task": "task 1"}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result1.is_ok());

    // Task 2
    let result2 = ctx.registry.execute(
        "opencode",
        r#"{"task": "task 2"}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result2.is_ok());

    // Task 3
    let result3 = ctx.registry.execute(
        "opencode",
        r#"{"task": "task 3"}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result3.is_ok());

    teardown(ctx).await;
}
```

---

## Sandbox интеграционные тесты

### Тест 1: Полный workflow с Sandbox

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_full_sandbox_workflow() {
    let ctx = setup().await;

    // Выполнить команду
    let args = r#"{"command": "echo 'Hello, World!'"}"#;
    let result = ctx.registry.execute("execute_command", args, &ctx.progress_tx, None).await;

    assert!(result.is_ok(), "Command should execute successfully");

    // Проверить результат
    let output = result.unwrap();
    assert!(output.contains("Hello, World!"));

    // Проверить прогресс события
    let tool_call = ctx.progress_rx.recv().await.unwrap();
    matches!(tool_call, AgentEvent::ToolCall { name, .. } if name == "execute_command");

    teardown(ctx).await;
}
```

### Тест 2: Несколько команд подряд

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_multiple_sandbox_commands() {
    let ctx = setup().await;

    // Command 1
    let result1 = ctx.registry.execute(
        "execute_command",
        r#"{"command": "echo 'test1'}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result1.is_ok());

    // Command 2
    let result2 = ctx.registry.execute(
        "execute_command",
        r#"{"command": "echo 'test2'}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result2.is_ok());

    // Command 3
    let result3 = ctx.registry.execute(
        "execute_command",
        r#"{"command": "echo 'test3'}"#,
        &ctx.progress_tx,
        None
    ).await;
    assert!(result3.is_ok());

    teardown(ctx).await;
}
```

---

## End-to-end тесты

### Тест 1: Комбинированный workflow (Sandbox + Opencode)

```rust
#[tokio::test]
#[ignore = "Requires Opencode server and Docker"]
async fn test_combined_workflow() {
    let ctx = setup().await;

    // Шаг 1: Скачать данные в Sandbox
    let download_args = r#"{"command": "echo 'Mock data' > /workspace/data.json"}"#;
    let download_result = ctx.registry.execute(
        "execute_command",
        download_args,
        &ctx.progress_tx,
        None
    ).await;
    assert!(download_result.is_ok(), "Data download should succeed");

    // Шаг 2: Обработать данные в Opencode
    let opencode_args = r#"{"task": "process data.json and update API"}"#;
    let opencode_result = ctx.registry.execute(
        "opencode",
        opencode_args,
        &ctx.progress_tx,
        None
    ).await;
    assert!(opencode_result.is_ok(), "Opencode task should succeed");

    // Проверить прогресс события (должно быть 2 tool calls)
    let event1 = ctx.progress_rx.recv().await.unwrap();
    matches!(event1, AgentEvent::ToolCall { name, .. } if name == "execute_command");

    let event2 = ctx.progress_rx.recv().await.unwrap();
    matches!(event2, AgentEvent::ToolCall { name, .. } if name == "opencode");

    teardown(ctx).await;
}
```

### Тест 2: Error handling

```rust
#[tokio::test]
async fn test_error_handling() {
    let ctx = setup().await;

    // Попытка выполнить с недоступным Opencode
    let wrong_url = "http://localhost:9999";
    let wrong_registry = ToolRegistry::new(Some(wrong_url.to_string()));

    let args = r#"{"task": "test"}"#;
    let result = wrong_registry.execute("opencode", args, &ctx.progress_tx, None).await;

    assert!(result.is_err(), "Should return error");
    assert!(result.unwrap_err().contains("Failed to connect") || result.unwrap_err().contains("connection refused"));

    teardown(ctx).await;
}
```

---

## Performance тесты

### Тест 1: Время выполнения Opencode задачи

```rust
#[tokio::test]
#[ignore = "Requires Opencode server"]
async fn test_opencode_performance() {
    let ctx = setup().await;

    let start = std::time::Instant::now();

    let result = ctx.registry.execute(
        "opencode",
        r#"{"task": "list files"}"#,
        &ctx.progress_tx,
        None
    ).await;

    let duration = start.elapsed();

    assert!(result.is_ok(), "Task should execute successfully");
    assert!(duration < Duration::from_secs(30), "Should complete within 30s");

    println!("Opencode task completed in {:?}", duration);

    teardown(ctx).await;
}
```

### Тест 2: Время выполнения Sandbox команды

```rust
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_sandbox_performance() {
    let ctx = setup().await;

    let start = std::time::Instant::now();

    let result = ctx.registry.execute(
        "execute_command",
        r#"{"command": "echo 'test'}"#,
        &ctx.progress_tx,
        None
    ).await;

    let duration = start.elapsed();

    assert!(result.is_ok(), "Command should execute successfully");
    assert!(duration < Duration::from_secs(5), "Should complete within 5s");

    println!("Sandbox command completed in {:?}", duration);

    teardown(ctx).await;
}
```

---

## Запуск интеграционных тестов

### Все интеграционные тесты

```bash
# Запустить все интеграционные тесты
cargo test --test integration

# С verbose выводом
cargo test --test integration -- --nocapture

# Параллельно (внимание: может быть проблематично для тестов с Docker)
cargo test --test integration -- --test-threads=2
```

### Конкретный тест

```bash
# Запустить конкретный интеграционный тест
cargo test test_full_opencode_workflow --test integration

# Запустить все тесты в файле
cargo test integration_tests --test integration
```

### CI/CD

```yaml
# .github/workflows/integration-tests.yml
name: Integration Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest

    services:
      opencode:
        image: opencode:latest
        ports:
          - 4096:4096
        options: >-
          --health-cmd "curl -f http://localhost:4096/vcs"
          --health-interval 30s
          --health-timeout 10s
          --health-retries 5

    steps:
      - uses: actions/checkout@v3

      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Run integration tests
        run: cargo test --test integration -- --ignored

      - name: Upload logs
        if: failure()
        uses: actions/upload-artifact@v3
        with:
          name: test-logs
          path: logs/
```

---

## Benchmarks

### Criterion для бенчмарков

Добавить в `Cargo.toml`:

```toml
[dev-dependencies]
criterion = "0.5"

[[bench]]
name = "opencode_bench"
harness = false
```

### Бенчмарк Opencode

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_opencode_execute_task(c: &mut Criterion) {
    let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

    c.bench_function("execute_task", |b| {
        b.iter(|| {
            black_box(
                provider.execute_task("list files")
            )
        })
    });
}

criterion_group!(benches, bench_opencode_execute_task);
criterion_main!(benches);
```

### Запуск бенчмарков

```bash
# Запустить бенчмарки
cargo bench

# Сохранить baseline
cargo bench -- --save-baseline main

# Сравнить с baseline
cargo bench -- --baseline main
```

---

## Следующие шаги

- [ ] Изучить [troubleshooting.md](./troubleshooting.md) - Устранение проблем
- [ ] Изучить [deployment/production_checklist.md](../deployment/production_checklist.md) - Чек-лист
- [ ] Перейти к [deployment/](../deployment/) - Деплой

---

**Связанные документы:**

- [testing/unit_tests.md](./unit_tests.md) - Unit тесты
- [testing/troubleshooting.md](./troubleshooting.md) - Устранение проблем
- [examples/integration_examples.rs](../examples/integration_examples.rs) - Примеры кода
