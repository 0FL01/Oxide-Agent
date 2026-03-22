# Opencode + Sandbox Integration - Documentation Index

> **Индекс всех файлов с описанием и ссылками**
>
> 📁 **Структура:** Полная независимая документация
> 🚀 **Статус:** Production Ready

---

## 📋 Навигация

- [Быстрый поиск](#быстрый-поиск)
- [По типу контента](#по-типу-контента)
- [Полный список файлов](#полный-список-файлов)
- [Порядок чтения](#порядок-чтения)

---

## 🔍 Быстрый поиск

| Хотите найти?              | Перейдите к                                                  |
| -------------------------- | ------------------------------------------------------------ |
| **Как начать?**            | [README.md](./README.md)                                     |
| **Архитектура системы?**   | [architecture/overview.md](./architecture/overview.md)       |
| **Код для копирования?**   | [implementation/](./implementation/)                         |
| **Примеры использования?** | [examples/](./examples/)                                     |
| **Как настроить?**         | [configuration/setup.sh](./configuration/setup.sh)           |
| **LLM prompt?**            | [configuration/llm_prompt.md](./configuration/llm_prompt.md) |
| **Тестирование?**          | [testing/](./testing/)                                       |
| **Проблемы?**              | [testing/troubleshooting.md](./testing/troubleshooting.md)   |
| **Деплой?**                | [deployment/](./deployment/)                                 |

---

## 📂 По типу контента

### 📖 Документация (Markdown)

| Файл                                                                               | Описание                    | Для кого     | Читаемость |
| ---------------------------------------------------------------------------------- | --------------------------- | ------------ | ---------- |
| [README.md](./README.md)                                                           | Главная документация, обзор | Все          | ⭐⭐⭐⭐⭐ |
| [architecture/overview.md](./architecture/overview.md)                             | Обзор архитектуры           | Разработчики | ⭐⭐⭐⭐   |
| [architecture/flow.md](./architecture/flow.md)                                     | Потоки выполнения           | Разработчики | ⭐⭐⭐     |
| [architecture/components.md](./architecture/components.md)                         | Компоненты системы          | Разработчики | ⭐⭐⭐⭐   |
| [examples/basic_usage.md](./examples/basic_usage.md)                               | Базовое использование       | Новички      | ⭐⭐⭐⭐⭐ |
| [examples/advanced_workflow.md](./examples/advanced_workflow.md)                   | Сложные рабочие процессы    | Опытные      | ⭐⭐⭐⭐   |
| [configuration/environment_variables.md](./configuration/environment_variables.md) | Переменные окружения        | DevOps       | ⭐⭐⭐     |
| [testing/unit_tests.md](./testing/unit_tests.md)                                   | Unit тесты                  | Разработчики | ⭐⭐⭐     |
| [testing/integration_tests.md](./testing/integration_tests.md)                     | Интеграционные тесты        | Разработчики | ⭐⭐⭐     |
| [testing/troubleshooting.md](./testing/troubleshooting.md)                         | Устранение проблем          | Все          | ⭐⭐⭐⭐⭐ |
| [deployment/production_checklist.md](./deployment/production_checklist.md)         | Чек-лист для продакшена     | DevOps       | ⭐⭐⭐⭐⭐ |
| [deployment/monitoring.md](./deployment/monitoring.md)                             | Мониторинг и логирование    | DevOps       | ⭐⭐⭐⭐   |

### 💻 Код (Rust)

| Файл                                                                               | Описание                  | Зависимости        | Сложность |
| ---------------------------------------------------------------------------------- | ------------------------- | ------------------ | --------- |
| [implementation/opencode_provider.rs](./implementation/opencode_provider.rs)       | HTTP клиент для Opencode  | reqwest, serde     | ⭐⭐⭐    |
| [implementation/registry_integration.rs](./implementation/registry_integration.rs) | Интеграция в ToolRegistry | tokio, async-trait | ⭐⭐⭐    |
| [implementation/session.rs](./implementation/session.rs)                           | Управление сессиями       | tokio              | ⭐⭐      |
| [examples/integration_examples.rs](./examples/integration_examples.rs)             | 6 практических примеров   | tokio              | ⭐⭐      |

### ⚙️ Скрипты (Bash)

| Файл                                               | Описание                 | Зависимости |
| -------------------------------------------------- | ------------------------ | ----------- |
| [configuration/setup.sh](./configuration/setup.sh) | Автоматическая настройка | curl, git   |

### 🤖 Конфигурации (Markdown)

| Файл                                                         | Описание              | Использование     |
| ------------------------------------------------------------ | --------------------- | ----------------- |
| [configuration/llm_prompt.md](./configuration/llm_prompt.md) | System prompt для LLM | Скопировать в LLM |

---

## 📁 Полный список файлов

### Корневые файлы

```
opencode-sandbox-integration/
├── README.md                  # Главная документация (НАЧАТЬ ЗДЕСЬ)
└── INDEX.md                   # Этот файл (индекс)
```

### Архитектура

```
architecture/
├── overview.md                # Обзор архитектуры системы
├── flow.md                   # Потоки выполнения с примерами
└── components.md             # Детальное описание компонентов
```

### Реализация

```
implementation/
├── opencode_provider.rs      # OpencodeToolProvider (HTTP клиент)
├── registry_integration.rs   # ToolRegistry интеграция
└── session.rs              # Session управление
```

### Примеры

```
examples/
├── basic_usage.md           # Базовое использование (3 примера)
├── advanced_workflow.md     # Сложные рабочие процессы (3 примера)
└── integration_examples.rs # 6 практических примеров на Rust
```

### Конфигурация

```
configuration/
├── llm_prompt.md          # System prompt для LLM агента
├── setup.sh               # Скрипт автоматической настройки
└── environment_variables.md # Описание переменных окружения
```

### Тестирование

```
testing/
├── unit_tests.md          # Unit тесты (описание)
├── integration_tests.md   # Интеграционные тесты (описание)
└── troubleshooting.md    # Устранение проблем (FAQ)
```

### Деплой

```
deployment/
├── production_checklist.md # Чек-лист для продакшена
└── monitoring.md         # Мониторинг и логирование
```

---

## 📖 Порядок чтения

### Путь для новичков (рекомендуется)

1. 📖 [README.md](./README.md) - Главный обзор
2. 🏗️ [architecture/overview.md](./architecture/overview.md) - Понять архитектуру
3. 🚀 [configuration/setup.sh](./configuration/setup.sh) - Настроить систему
4. 💻 [examples/basic_usage.md](./examples/basic_usage.md) - Базовые примеры
5. 🧪 [testing/troubleshooting.md](./testing/troubleshooting.md) - Если возникнут проблемы

**Время:** ~30 минут

---

### Путь для разработчиков

1. 📖 [README.md](./README.md) - Главный обзор
2. 🏗️ [architecture/overview.md](./architecture/overview.md) - Архитектура
3. 🏗️ [architecture/flow.md](./architecture/flow.md) - Потоки выполнения
4. 🏗️ [architecture/components.md](./architecture/components.md) - Компоненты
5. 💻 [implementation/opencode_provider.rs](./implementation/opencode_provider.rs) - Код провайдера
6. 💻 [implementation/registry_integration.rs](./implementation/registry_integration.rs) - Интеграция
7. 🧪 [testing/unit_tests.md](./testing/unit_tests.md) - Unit тесты
8. 🧪 [testing/integration_tests.md](./testing/integration_tests.md) - Интеграционные тесты

**Время:** ~1 час

---

### Путь для DevOps

1. 📖 [README.md](./README.md) - Главный обзор
2. ⚙️ [configuration/setup.sh](./configuration/setup.sh) - Настройка
3. ⚙️ [configuration/environment_variables.md](./configuration/environment_variables.md) - Переменные окружения
4. 🚀 [deployment/production_checklist.md](./deployment/production_checklist.md) - Чек-лист
5. 📊 [deployment/monitoring.md](./deployment/monitoring.md) - Мониторинг
6. 🧪 [testing/troubleshooting.md](./testing/troubleshooting.md) - Проблемы

**Время:** ~45 минут

---

### Путь для ML инженеров

1. 📖 [README.md](./README.md) - Главный обзор
2. 🏗️ [architecture/overview.md](./architecture/overview.md) - Архитектура
3. 🏗️ [architecture/flow.md](./architecture/flow.md) - Потоки выполнения
4. 🤖 [configuration/llm_prompt.md](./configuration/llm_prompt.md) - LLM prompt
5. 💻 [examples/integration_examples.rs](./examples/integration_examples.rs) - Примеры кода
6. 📚 [examples/advanced_workflow.md](./examples/advanced_workflow.md) - Сложные сценарии

**Время:** ~45 минут

---

## 🎯 Быстрые ссылки

### Хочу:

- **Начать сейчас** → [README.md](./README.md)
- **Понять как работает** → [architecture/overview.md](./architecture/overview.md)
- **Скопировать код** → [implementation/](./implementation/)
- **Видеть примеры** → [examples/](./examples/)
- **Настроить LLM** → [configuration/llm_prompt.md](./configuration/llm_prompt.md)
- **Запустить сейчас** → [configuration/setup.sh](./configuration/setup.sh)
- **Тестировать** → [testing/](./testing/)
- **Решить проблему** → [testing/troubleshooting.md](./testing/troubleshooting.md)
- **Деплоить** → [deployment/](./deployment/)

---

## 📊 Статистика документации

| Метрика                    | Значение |
| -------------------------- | -------- |
| **Всего файлов**           | 18       |
| **Markdown файлов**        | 13       |
| **Rust файлов**            | 4        |
| **Bash скриптов**          | 1        |
| **Общее количество строк** | ~3500    |
| **Примеров кода**          | 9        |
| **Диаграмм**               | 5        |

---

## 🔗 Связанная документация

### В проекте Opencode

Если вы копируете это в репозиторий Opencode:

- [TELEGRAM_OPCODE_INTEGRATION.md](../../TELEGRAM_OPCODE_INTEGRATION.md) - Telegram бот интеграция
- [OPCODE_CLI_ARCHITECT_SUMMARY.md](../../OPCODE_CLI_ARCHITECT_SUMMARY.md) - Резюме исследования

### Внешние ресурсы

- [Opencode Docs](https://opencode.ai/docs)
- [Rust Async Book](https://rust-lang.github.io/async-book/)
- [reqwest Docs](https://docs.rs/reqwest/)

---

## 📝 Примечания по использованию

### Перенос в ваш проект

Чтобы использовать эту документацию в вашем проекте:

1. Скопируйте всю папку `opencode-sandbox-integration/`
2. Обновите ссылки на внешние ресурсы (если есть)
3. Обновите раздел "Связанная документация"
4. Настройте навигацию для вашего проекта

### Независимость

Эта документация полностью независима:

- ✅ Все файлы находятся в одной папке
- ✅ Нет внешних ссылок на контент
- ✅ Все ссылки относительные
- ✅ Можно скопировать в любой проект

### Обновления

Для обновления документации:

1. Обновите файлы в папке `opencode-sandbox-integration/`
2. Обновите версию в [README.md](./README.md)
3. Обновите дату в [README.md](./README.md)
4. Проверьте все ссылки

---

## 🆘 Где получить помощь

1. **Начните с:** [testing/troubleshooting.md](./testing/troubleshooting.md)
2. **Посмотрите примеры:** [examples/](./examples/)
3. **Изучите архитектуру:** [architecture/](./architecture/)
4. **Проверьте конфигурацию:** [configuration/](./configuration/)

---

**Начните здесь:** [README.md](./README.md)

**Или выберите свой путь** в разделе "Порядок чтения" выше.
