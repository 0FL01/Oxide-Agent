# Documentation Index

> **Индекс всей документации проекта Opencode**
>
> 📁 **Расположение:** `/home/stfu/ai/opencode/documentation/`

---

## 📋 Основная документация

### Opencode + Sandbox Integration (ПОЛНАЯ НЕЗАВИСИМАЯ ДОКУМЕНТАЦИЯ)

**Статус:** ✅ Production Ready

Это **основная документация**, которую вы можете полностью скопировать в свой проект.

| Файл          | Описание                                 | Ссылка                                              |
| ------------- | ---------------------------------------- | --------------------------------------------------- |
| **README.md** | Главная страница документации интеграции | [Перейти](./opencode-sandbox-integration/README.md) |
| **INDEX.md**  | Индекс всех файлов документации          | [Перейти](./opencode-sandbox-integration/INDEX.md)  |

**Структура:**

```
opencode-sandbox-integration/
├── README.md              # Главная страница
├── INDEX.md               # Индекс файлов
├── architecture/           # Архитектура (3 файла)
├── implementation/        # Код (3 файла)
├── examples/              # Примеры (3 файла)
├── configuration/         # Конфигурация (3 файла)
├── testing/              # Тестирование (3 файла)
└── deployment/           # Деплой (2 файла)
```

**Как использовать:**

```bash
# Скопировать в ваш проект
cp -r opencode-sandbox-integration /path/to/your/project/docs/

# Или просто прочитать онлайн
open opencode-sandbox-integration/README.md
```

---

## 📁 Все папки и файлы

### architecture/ (3 файла)

| Файл                                                                       | Описание                    | Для кого     |
| -------------------------------------------------------------------------- | --------------------------- | ------------ |
| [overview.md](./opencode-sandbox-integration/architecture/overview.md)     | Обзор архитектуры системы   | Разработчики |
| [flow.md](./opencode-sandbox-integration/architecture/flow.md)             | Детальные потоки выполнения | Разработчики |
| [components.md](./opencode-sandbox-integration/architecture/components.md) | Описание всех компонентов   | Разработчики |

---

### implementation/ (3 файла)

| Файл                                                                                             | Описание                  | Язык |
| ------------------------------------------------------------------------------------------------ | ------------------------- | ---- |
| [opencode_provider.rs](./opencode-sandbox-integration/implementation/opencode_provider.rs)       | HTTP клиент для Opencode  | Rust |
| [registry_integration.rs](./opencode-sandbox-integration/implementation/registry_integration.rs) | Интеграция в ToolRegistry | Rust |
| [session.rs](./opencode-sandbox-integration/implementation/session.rs)                           | Управление сессиями       | Rust |

---

### examples/ (3 файла)

| Файл                                                                                       | Описание                        | Для кого     |
| ------------------------------------------------------------------------------------------ | ------------------------------- | ------------ |
| [basic_usage.md](./opencode-sandbox-integration/examples/basic_usage.md)                   | Базовые примеры использования   | Новички      |
| [advanced_workflow.md](./opencode-sandbox-integration/examples/advanced_workflow.md)       | Сложные рабочие процессы        | Опытные      |
| [integration_examples.rs](./opencode-sandbox-integration/examples/integration_examples.rs) | 6 практических примеров на Rust | Разработчики |

---

### configuration/ (3 файла)

| Файл                                                                                              | Описание                        | Использование |
| ------------------------------------------------------------------------------------------------- | ------------------------------- | ------------- |
| [llm_prompt.md](./opencode-sandbox-integration/configuration/llm_prompt.md)                       | System prompt для LLM агента    | Скопировать   |
| [setup.sh](./opencode-sandbox-integration/configuration/setup.sh)                                 | Скрипт автоматической настройки | Запустить     |
| [environment_variables.md](./opencode-sandbox-integration/configuration/environment_variables.md) | Описание всех переменных        | Читать        |

---

### testing/ (3 файла)

| Файл                                                                                | Описание                 | Для кого     |
| ----------------------------------------------------------------------------------- | ------------------------ | ------------ |
| [unit_tests.md](./opencode-sandbox-integration/testing/unit_tests.md)               | Unit тесты и mocking     | Разработчики |
| [integration_tests.md](./opencode-sandbox-integration/testing/integration_tests.md) | Интеграционные тесты     | Разработчики |
| [troubleshooting.md](./opencode-sandbox-integration/testing/troubleshooting.md)     | Устранение проблем и FAQ | Все          |

---

### deployment/ (2 файла)

| Файл                                                                                         | Описание                         | Для кого |
| -------------------------------------------------------------------------------------------- | -------------------------------- | -------- |
| [production_checklist.md](./opencode-sandbox-integration/deployment/production_checklist.md) | Чек-лист для деплоя в продакшен  | DevOps   |
| [monitoring.md](./opencode-sandbox-integration/deployment/monitoring.md)                     | Мониторинг, логирование и alerts | DevOps   |

---

## 🔍 Поиск

### По типу контента

#### 📖 Markdown документация (13 файлов)

- [architecture/overview.md](./opencode-sandbox-integration/architecture/overview.md)
- [architecture/flow.md](./opencode-sandbox-integration/architecture/flow.md)
- [architecture/components.md](./opencode-sandbox-integration/architecture/components.md)
- [examples/basic_usage.md](./opencode-sandbox-integration/examples/basic_usage.md)
- [examples/advanced_workflow.md](./opencode-sandbox-integration/examples/advanced_workflow.md)
- [configuration/environment_variables.md](./opencode-sandbox-integration/configuration/environment_variables.md)
- [testing/unit_tests.md](./opencode-sandbox-integration/testing/unit_tests.md)
- [testing/integration_tests.md](./opencode-sandbox-integration/testing/integration_tests.md)
- [testing/troubleshooting.md](./opencode-sandbox-integration/testing/troubleshooting.md)
- [deployment/production_checklist.md](./opencode-sandbox-integration/deployment/production_checklist.md)
- [deployment/monitoring.md](./opencode-sandbox-integration/deployment/monitoring.md)
- [opencode-sandbox-integration/INDEX.md](./opencode-sandbox-integration/INDEX.md)
- [opencode-sandbox-integration/README.md](./opencode-sandbox-integration/README.md)

#### 💻 Код на Rust (4 файла)

- [implementation/opencode_provider.rs](./opencode-sandbox-integration/implementation/opencode_provider.rs)
- [implementation/registry_integration.rs](./opencode-sandbox-integration/implementation/registry_integration.rs)
- [implementation/session.rs](./opencode-sandbox-integration/implementation/session.rs)
- [examples/integration_examples.rs](./opencode-sandbox-integration/examples/integration_examples.rs)

#### ⚙️ Скрипты (1 файл)

- [configuration/setup.sh](./opencode-sandbox-integration/configuration/setup.sh)

---

## 🚀 Быстрый доступ

### Для новичков

1. [README.md](./opencode-sandbox-integration/README.md) - Главный обзор
2. [architecture/overview.md](./opencode-sandbox-integration/architecture/overview.md) - Архитектура
3. [examples/basic_usage.md](./opencode-sandbox-integration/examples/basic_usage.md) - Базовые примеры
4. [testing/troubleshooting.md](./opencode-sandbox-integration/testing/troubleshooting.md) - Проблемы

### Для разработчиков

1. [README.md](./opencode-sandbox-integration/README.md) - Главный обзор
2. [architecture/](./opencode-sandbox-integration/architecture/) - Вся архитектура
3. [implementation/](./opencode-sandbox-integration/implementation/) - Код
4. [testing/](./opencode-sandbox-integration/testing/) - Тестирование

### Для DevOps

1. [README.md](./opencode-sandbox-integration/README.md) - Главный обзор
2. [configuration/](./opencode-sandbox-integration/configuration/) - Конфигурация
3. [deployment/](./opencode-sandbox-integration/deployment/) - Деплой
4. [testing/troubleshooting.md](./opencode-sandbox-integration/testing/troubleshooting.md) - Проблемы

---

## 📊 Статистика

| Категория                  | Количество                                         |
| -------------------------- | -------------------------------------------------- |
| **Папок**                  | 2 (documentation/ + opencode-sandbox-integration/) |
| **Markdown файлов**        | 13                                                 |
| **Rust файлов**            | 4                                                  |
| **Bash скриптов**          | 1                                                  |
| **Всего файлов**           | 18                                                 |
| **Общее количество строк** | ~4000                                              |
| **Примеров кода**          | 9                                                  |
| **Диаграмм**               | 3                                                  |

---

## ✅ Перенос в ваш проект

### Для копирования всей документации

```bash
# В корне вашего проекта
mkdir -p docs/opencode-sandbox-integration

# Скопировать все файлы
cp -r /path/to/opencode/documentation/opencode-sandbox-integration/* docs/opencode-sandbox-integration/
```

### Для интеграции с существующей документацией

Добавьте в ваш главный `README.md`:

```markdown
## Documentation

### Opencode Integration

Полная документация по интеграции Opencode с существующей системой песочницы.

- [**Начать здесь**](docs/opencode-sandbox-integration/README.md) - Главный обзор
- [**Индекс**](docs/opencode-sandbox-integration/INDEX.md) - Все файлы

**Пути:**

- 📖 [Архитектура](docs/opencode-sandbox-integration/architecture/)
- 💻 [Реализация](docs/opencode-sandbox-integration/implementation/)
- 📚 [Примеры](docs/opencode-sandbox-integration/examples/)
- ⚙️ [Конфигурация](docs/opencode-sandbox-integration/configuration/)
- 🧪 [Тестирование](docs/opencode-sandbox-integration/testing/)
- 🚀 [Деплой](docs/opencode-sandbox-integration/deployment/)
```

---

## 🎯 Начать здесь

**Для всех:**

- 📖 [opencode-sandbox-integration/README.md](./opencode-sandbox-integration/README.md) - **ГЛАВНАЯ СТРАНИЦА**
- 🗂️ [opencode-sandbox-integration/INDEX.md](./opencode-sandbox-integration/INDEX.md) - Индекс всех файлов

**Для поиска конкретного файла:**

- Используйте раздел "📁 Все папки и файлы" выше
- Или используйте раздел "🔍 Поиск" для поиска по типу контента

---

**Документация готова к использованию!** 🎉
