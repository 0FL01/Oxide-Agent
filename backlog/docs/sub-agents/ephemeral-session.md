# EphemeralSession

Изолированная сессия для выполнения саб-агента.

## Назначение

Обеспечивает изоляцию саб-агента от основного агента:
- Отдельная память (`AgentMemory`)
- Отдельный токен отмены (child token)
- Лимит токенов для предотвращения переполнения контекста

## Структура

```rust
// src/agent/context.rs:28-35
pub struct EphemeralSession {
    memory: AgentMemory,
    cancellation_token: CancellationToken,
    loaded_skills: HashSet<String>,
    skill_token_count: usize,
    started_at: std::time::Instant,
}
```

## Поля

| Поле | Тип | Описание |
|------|------|----------|
| `memory` | AgentMemory | Память агента с ограничением SUB_AGENT_MAX_TOKENS |
| `cancellation_token` | CancellationToken | Токен отмены (child от родителя или новый) |
| `loaded_skills` | HashSet<String> | Загруженные навыки (для RAG) |
| `skill_token_count` | usize | Токены, использованные на навыки |
| `started_at` | Instant | Время запуска для elapsed_secs() |

## Конструкторы

### new()

```rust
// src/agent/context.rs:39-48
#[must_use]
pub fn new(max_tokens: usize) -> Self {
    Self {
        memory: AgentMemory::new(max_tokens),
        cancellation_token: CancellationToken::new(),
        loaded_skills: HashSet::new(),
        skill_token_count: 0,
        started_at: std::time::Instant::now(),
    }
}
```

Создаёт новую изолированную сессию с собственным токеном отмены.

### with_parent_token()

```rust
// src/agent/context.rs:54-62
#[must_use]
pub fn with_parent_token(max_tokens: usize, parent: &CancellationToken) -> Self {
    Self {
        memory: AgentMemory::new(max_tokens),
        cancellation_token: parent.child_token(),  // При отмене родителя отменяется и саб-агент
        loaded_skills: HashSet::new(),
        skill_token_count: 0,
        started_at: std::time::Instant::now(),
    }
}
```

Создаёт сессию с child токеном, связанным с родительским.

### with_default_limits()

```rust
// src/agent/context.rs:66-69
#[must_use]
pub fn with_default_limits() -> Self {
    Self::new(AGENT_MAX_TOKENS)
}
```

Создаёт сессию с лимитами основного агента.

## Реализация AgentContext

```rust
// src/agent/context.rs:109-138
impl AgentContext for EphemeralSession {
    fn memory(&self) -> &AgentMemory {
        &self.memory
    }

    fn memory_mut(&mut self) -> &mut AgentMemory {
        &mut self.memory
    }

    fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }

    fn is_skill_loaded(&self, name: &str) -> bool {
        self.loaded_skills.contains(name)
    }

    fn register_loaded_skill(&mut self, name: &str, token_count: usize) -> bool {
        if self.loaded_skills.insert(name.to_string()) {
            self.skill_token_count = self.skill_token_count.saturating_add(token_count);
            return true;
        }
        false
    }

    fn elapsed_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
```

## Child Token

При создании саб-агента используется `parent.child_token()`:

```rust
// src/agent/providers/delegation.rs:245-250
let mut sub_session = match cancellation_token {
    Some(parent_token) => {
        EphemeralSession::with_parent_token(SUB_AGENT_MAX_TOKENS, parent_token)
    }
    None => EphemeralSession::new(SUB_AGENT_MAX_TOKENS),
};
```

### Свойства child token

- **Cascade cancellation:** При отмене родительского токена, автоматически отменяется child token
- **Independent operations:** Child token может быть отменён самостоятельно
- **Loop detection:** При детектировании зацикливания отменяется родитель, отменяя всех саб-агентов

## Пример использования

### Создание с родительским токеном
```rust
let parent_token = CancellationToken::new();
let sub_session = EphemeralSession::with_parent_token(64000, &parent_token);

// При отмене родителя:
parent_token.cancel();

// Саб-агент также будет отменён
let is_cancelled = sub_session.cancellation_token().is_cancelled();  // true
```

### Создание без родителя
```rust
let sub_session = EphemeralSession::new(64000);

// Независимый токен отмены
sub_session.cancellation_token_mut().cancel();
```

## Работа с памятью

### Добавление сообщений
```rust
sub_session
    .memory_mut()
    .add_message(AgentMessage::user(task.as_str()));
```

### Доступ к сообщениям
```rust
let messages = AgentRunner::convert_memory_to_messages(
    sub_session.memory().get_messages()
);
```

### Подсчёт токенов
```rust
let token_count = sub_session.memory().token_count();
```

## Отслеживание навыков

### Проверка загрузки
```rust
if !sub_session.is_skill_loaded("my-skill") {
    // Загрузить навык
}
```

### Регистрация загрузки
```rust
if sub_session.register_loaded_skill("my-skill", token_count) {
    // Навык был загружен первый раз
} else {
    // Навык уже был загружен
}
```

### Счётчик токенов навыков
```rust
let skill_tokens = sub_session.skill_token_count();
```

## Лимиты саб-агента

| Параметр | Значение | Константа |
|-----------|----------|-----------|
| `max_tokens` | 64,000 | `SUB_AGENT_MAX_TOKENS` |
| `timeout` | настроенный | `AGENT_TIMEOUT_SECS` (для main), отдельный для sub |

## Изоляция

### Память
- `AgentMemory` создаётся с `SUB_AGENT_MAX_TOKENS`
- Не разделяется с основным агентом
- Не влияет на контекст основного агента

### Токены отмены
- `with_parent_token()` создаёт зависимость от родителя
- `new()` создаёт независимый токен

### Время
- `started_at` фиксируется при создании
- `elapsed_secs()` возвращает время с момента создания

## Сравнение с AgentSession

| Характеристика | AgentSession | EphemeralSession |
|---------------|-------------|-------------------|
| Использование | Основной агент | Саб-агенты |
| `cancellation_token` | Собственный | Child от родителя или собственный |
| `max_tokens` | `AGENT_MAX_TOKENS` (200,000) | `SUB_AGENT_MAX_TOKENS` (64,000) |
| Навыки | Персистентные | Временные в сессии |
| `elapsed_secs()` | От создания session | От создания EphemeralSession |
