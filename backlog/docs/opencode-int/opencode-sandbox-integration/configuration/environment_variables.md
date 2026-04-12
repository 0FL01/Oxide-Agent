# Environment Variables

> **Переменные окружения для конфигурации интеграции**
>
> 📁 **Раздел:** Configuration
> 🎯 **Цель:** Понять как настроить систему

---

## 📋 Оглавление

- [Opencode переменные](#opencode-переменные)
- [Sandbox переменные](#sandbox-переменные)
- [LLM переменные](#llm-переменные)
- [Системные переменные](#системные-переменные)
- [Пример конфигурации](#пример-конфигурации)

---

## Opencode переменные

### OPENCODE_BASE_URL

**Описание:** URL Opencode Server

**По умолчанию:** `http://127.0.0.1:4096`

**Примеры:**

```bash
# Локальный сервер
export OPENCODE_BASE_URL=http://127.0.0.1:4096

# Удаленный сервер
export OPENCODE_BASE_URL=http://opencode.example.com:4096

# HTTPS
export OPENCODE_BASE_URL=https://opencode.example.com
```

**Использование:**

```rust
let url = std::env::var("OPENCODE_BASE_URL")
    .unwrap_or_else(|_| "http://127.0.0.1:4096".to_string());

let provider = OpencodeToolProvider::new(url);
```

---

### OPENCODE_TIMEOUT

**Описание:** Таймаут для запросов к Opencode (в секундах)

**По умолчанию:** `300` (5 минут)

**Примеры:**

```bash
# Стандартный (5 минут)
export OPENCODE_TIMEOUT=300

# Увеличенный (10 минут)
export OPENCODE_TIMEOUT=600

# Для быстрых задач (1 минута)
export OPENCODE_TIMEOUT=60
```

**Использование:**

```rust
let timeout_secs = std::env::var("OPENCODE_TIMEOUT")
    .ok()
    .and_then(|s| s.parse::<u64>().ok())
    .unwrap_or(300);

let timeout = Duration::from_secs(timeout_secs);

let provider = OpencodeToolProvider::new(url)
    .with_timeout(timeout);
```

---

## Sandbox переменные

### SANBOX_DOCKER_IMAGE

**Описание:** Docker образ для sandbox контейнера

**По умолчанию:** `agent-sandbox:latest`

**Примеры:**

```bash
# Стандартный образ
export SANBOX_DOCKER_IMAGE=agent-sandbox:latest

# Кастомный образ
export SANBOX_DOCKER_IMAGE=company/agent-sandbox:v1.2.3

# Local образ
export SANBOX_DOCKER_IMAGE=agent-sandbox-local:dev
```

---

### SANBOX_MEMORY_LIMIT

**Описание:** Лимит памяти для контейнера (в GB)

**По умолчанию:** `1`

**Примеры:**

```bash
# Стандартный (1GB)
export SANBOX_MEMORY_LIMIT=1

# Увеличенный (2GB)
export SANBOX_MEMORY_LIMIT=2

# Для тяжелых задач (4GB)
export SANBOX_MEMORY_LIMIT=4
```

---

### SANBOX_CPU_LIMIT

**Описание:** Лимит CPU для контейнера (в ядрах)

**По умолчанию:** `2`

**Примеры:**

```bash
# Стандартный (2 ядра)
export SANBOX_CPU_LIMIT=2

# Уменьшенный (1 ядро)
export SANBOX_CPU_LIMIT=1

# Для тяжелых задач (4 ядра)
export SANBOX_CPU_LIMIT=4
```

---

### SANBOX_WORKSPACE

**Описание:** Рабочая директория внутри контейнера

**По умолчанию:** `/workspace`

**Примеры:**

```bash
# Стандартный
export SANBOX_WORKSPACE=/workspace

# Кастомная
export SANBOX_WORKSPACE=/app
```

---

## LLM переменные

### LLM_MODEL

**Описание:** Модель LLM для использования

**По умолчанию:** `openai/gpt-4.1`

**Примеры:**

```bash
# GPT-4
export LLM_MODEL=openai/gpt-4.1

# GPT-3.5
export LLM_MODEL=openai/gpt-3.5-turbo

# Claude
export LLM_MODEL=anthropic/claude-3-opus

# Llama
export LLM_MODEL=meta/llama-3-70b
```

---

### LLM_TEMPERATURE

**Описание:** Temperature для LLM (креативность)

**По умолчанию:** `0.3`

**Диапазон:** `0.0` - `2.0`

**Примеры:**

```bash
# Низкая (детерминированная)
export LLM_TEMPERATURE=0.1

# Средняя
export LLM_TEMPERATURE=0.3

# Высокая (творческая)
export LLM_TEMPERATURE=0.7
```

**Рекомендации:**

- **0.0 - 0.3:** Для кода и технических задач
- **0.3 - 0.7:** Для анализа и объяснений
- **0.7 - 1.0:** Для творческих задач

---

### LLM_MAX_TOKENS

**Описание:** Максимальное количество токенов для ответа

**По умолчанию:** `4000`

**Примеры:**

```bash
# Стандартный
export LLM_MAX_TOKENS=4000

# Для длинных ответов
export LLM_MAX_TOKENS=8000

# Для коротких ответов
export LLM_MAX_TOKENS=1000
```

---

## Системные переменные

### RUST_LOG

**Описание:** Уровень логирования Rust

**По умолчанию:** `info`

**Примеры:**

```bash
# Минимальный
export RUST_LOG=error

# Стандартный
export RUST_LOG=info

# Отладка
export RUST_LOG=debug

# Максимальный
export RUST_LOG=trace
```

---

### LOG_FILE

**Описание:** Путь к файлу логов

**По умолчанию:** stdout (консоль)

**Примеры:**

```bash
# Логи в файл
export LOG_FILE=/var/log/agent.log

# Логи в специфичную директорию
export LOG_FILE=./logs/agent-$(date +%Y%m%d).log
```

---

## Пример конфигурации

### Для разработки

```bash
#!/bin/bash
# dev.env - Конфигурация для разработки

# Opencode
export OPENCODE_BASE_URL=http://127.0.0.1:4096
export OPENCODE_TIMEOUT=300

# Sandbox
export SANBOX_DOCKER_IMAGE=agent-sandbox:latest
export SANBOX_MEMORY_LIMIT=2
export SANBOX_CPU_LIMIT=2

# LLM
export LLM_MODEL=openai/gpt-4.1
export LLM_TEMPERATURE=0.3
export LLM_MAX_TOKENS=4000

# Логирование
export RUST_LOG=debug
export LOG_FILE=./logs/dev.log
```

### Для продакшена

```bash
#!/bin/bash
# prod.env - Конфигурация для продакшена

# Opencode
export OPENCODE_BASE_URL=http://opencode-internal:4096
export OPENCODE_TIMEOUT=600

# Sandbox
export SANBOX_DOCKER_IMAGE=company/agent-sandbox:prod
export SANBOX_MEMORY_LIMIT=1
export SANBOX_CPU_LIMIT=2

# LLM
export LLM_MODEL=anthropic/claude-3-opus
export LLM_TEMPERATURE=0.2
export LLM_MAX_TOKENS=8000

# Логирование
export RUST_LOG=info
export LOG_FILE=/var/log/agent.log
```

### Для тестирования

```bash
#!/bin/bash
# test.env - Конфигурация для тестирования

# Opencode
export OPENCODE_BASE_URL=http://127.0.0.1:4096
export OPENCODE_TIMEOUT=60

# Sandbox
export SANBOX_DOCKER_IMAGE=agent-sandbox:test
export SANBOX_MEMORY_LIMIT=1
export SANBOX_CPU_LIMIT=1

# LLM
export LLM_MODEL=openai/gpt-3.5-turbo
export LLM_TEMPERATURE=0.0
export LLM_MAX_TOKENS=1000

# Логирование
export RUST_LOG=trace
export LOG_FILE=./logs/test.log
```

---

## Загрузка переменных

### Из файла

```bash
# Загрузить переменные из файла
source dev.env

# Или
export $(cat dev.env | xargs)
```

### В коде Rust

```rust
use std::env;

// Чтение переменной
let base_url = env::var("OPENCODE_BASE_URL")
    .unwrap_or_else(|_| "http://127.0.0.1:4096".to_string());

let timeout = env::var("OPENCODE_TIMEOUT")
    .ok()
    .and_then(|s| s.parse::<u64>().ok())
    .unwrap_or(300);

// Проверка наличия переменной
if env::var("OPENCODE_BASE_URL").is_ok() {
    println!("Opencode URL настроен");
} else {
    println!("Используется URL по умолчанию");
}
```

### С помощью dotenv

Добавить в `Cargo.toml`:

```toml
[dependencies]
dotenv = "0.15"
```

Использование:

```rust
use dotenv::dotenv;

fn main() {
    // Загрузить переменные из .env файла
    dotenv().ok();

    // Использовать переменные
    let base_url = std::env::var("OPENCODE_BASE_URL").unwrap();
}
```

---

## Следующие шаги

- [ ] Изучить [llm_prompt.md](./llm_prompt.md) - System prompt для LLM
- [ ] Изучить [setup.sh](./setup.sh) - Скрипт настройки
- [ ] Перейти к [testing/](../testing/) - Тестирование

---

**Связанные документы:**

- [configuration/llm_prompt.md](./llm_prompt.md) - LLM prompt
- [configuration/setup.sh](./setup.sh) - Скрипт настройки
- [deployment/production_checklist.md](../deployment/production_checklist.md) - Чек-лист для продакшена
