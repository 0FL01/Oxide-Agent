
TL;DR: Skill System для Telegram AI-агента
Что делаем
Внедряем модульную систему контекста, аналогичную Claude Skills: разбиваем монолитный AGENT.md (~2000 токенов) на отдельные модули (web-search.md, video-processing.md, и т.д.) с метаданными и семантической активацией через Mistral Codestral Embed (256-dim векторы).

Hybrid Skill System с семантической активацией
📚 Источники архитектурных идей в проекте
Откуда брать паттерны:
1. Система провайдеров инструментов → src/agent/provider.rs + src/agent/registry.rs
   - Паттерн регистрации провайдеров
   - Trait-based архитектура
   - Динамический роутинг вызовов
2. Система памяти с токен-менеджментом → src/agent/memory.rs
   - Подсчёт токенов через tiktoken
   - Авто-компактация при превышении лимита
   - Прогрессивное управление контекстом
3. Hooks система → src/agent/hooks/
   - Паттерн событийных перехватчиков
   - Регистрация и диспетчеризация
   - Изоляция логики в отдельные модули
4. Loop Detection Service → src/agent/loop_detection/
   - Многофайловая модульная структура
   - Конфигурация через environment variables
   - Интеграция с executor через Arc<Mutex<>>
5. Метаданные из frontmatter → концепция из backlog/docs/skills/skills.md
   - YAML frontmatter для метаданных
   - Progressive disclosure паттерн
   - Model-invoked активация
🏗️ Структура директорий и файлов
Новые модули:
src/agent/skills/
├── mod.rs              # Публичный API, re-exports
├── types.rs            # Skill, SkillMetadata, SkillWeight, ActivationMode
├── loader.rs           # Парсинг .md файлов с YAML frontmatter
├── registry.rs         # SkillRegistry с embedding-based поиском
├── embeddings.rs       # Интеграция с Mistral codestral-embed
├── matcher.rs          # Семантическое и ключевое сопоставление
└── cache.rs            # Кэширование embeddings и загруженных скиллов
skills/                 # В корне проекта, рядом с AGENT.md
├── core.md            # Всегда загружается (базовые правила, форматирование)
├── web-search.md      # web_search, web_extract
├── video-processing.md # ytdlp_* инструменты
├── file-management.md # execute_command, write_file, read_file, send_file_to_user
├── file-hosting.md    # upload_file
└── task-planning.md   # write_todos
.embeddings_cache/     # Кэш векторов (gitignore)
└── skills/
    ├── web-search.bin
    ├── video-processing.bin
    └── ...
Модификации существующих файлов:
src/agent/
├── mod.rs              # Добавить pub mod skills;
├── session.rs          # Добавить skill_cache: SkillCache
├── executor.rs         # Модифицировать create_agent_system_prompt()
└── memory.rs           # Опционально: добавить поле для трекинга загруженных скиллов
src/llm/
├── mod.rs              # Добавить метод generate_embedding()
└── providers.rs        # Добавить Mistral embedding endpoint
src/config.rs           # Добавить SKILL_* константы
Cargo.toml              # Добавить зависимости (если нужны новые)
🧩 Компоненты системы
1. SkillMetadata (types.rs)
Назначение: Описание скилла без загрузки контента
Поля:
- name: Уникальный идентификатор скилла
- description: Семантическое описание для embedding-поиска
- triggers: Ключевые слова для быстрого матчинга (fallback)
- allowed_tools: Опциональный список инструментов (как в OpenCode)
- weight: SkillWeight (Always/High/Medium/OnDemand)
- references: Пути к supporting файлам (progressive disclosure)
- embedding: Опциональный кэшированный вектор (256-dim f32)
Источник идеи: Frontmatter из backlog/docs/skills/skills.md + метаданные из src/agent/loop_detection/types.rs
---
2. Skill (types.rs)
Назначение: Полная структура скилла с загруженным контентом
Поля:
- metadata: SkillMetadata
- content: Markdown контент после frontmatter
- supporting_files: HashMap<PathBuf, LazyContent> для progressive disclosure
- loaded_at: Timestamp для управления кэшем
- token_count: Размер контента в токенах
Источник идеи: Структура AgentMessage в src/agent/memory.rs
---
3. SkillRegistry (registry.rs)
Назначение: Центральный менеджер скиллов
Ключевые методы:
- load_all_metadata(): При старте бота - парсит все .md файлы в skills/, извлекает frontmatter
- get_embeddings(): Генерирует векторы для descriptions через Mistral API
- select_skills_semantic(): Принимает user_message, возвращает топ-N релевантных скиллов
- select_skills_by_tool(): Находит скилл по имени инструмента (из allowed_tools)
- load_skill_content(): Ленивая загрузка полного контента скилла
- invalidate_cache(): Сброс при изменении файлов (для dev режима)
Источник идеи: Паттерн из src/agent/registry.rs (ToolRegistry)
---
4. EmbeddingService (embeddings.rs)
Назначение: Интеграция с Mistral Codestral Embed
Ключевые методы:
- generate_embedding(): Вызов Mistral API для получения вектора
- batch_generate(): Пакетная генерация для всех скиллов
- cosine_similarity(): Вычисление схожести векторов
- load_from_cache() / save_to_cache(): Работа с .embeddings_cache/
Источник идеи: HTTP клиент из src/llm/http_utils.rs + паттерн провайдеров из src/llm/providers.rs
Endpoint: https://api.mistral.ai/v1/embeddings (model: mistral-embed)
---
5. SkillMatcher (matcher.rs)
Назначение: Гибридное сопоставление (семантика + триггеры)
Алгоритм:
1. Быстрый путь: Проверить триггеры (ключевые слова) - O(1)
2. Семантический путь: Если триггеры не сработали - embedding similarity
3. Tool-based путь: Если вызван инструмент - загрузить связанный скилл
Источник идеи: Логика детекции из src/agent/loop_detection/matcher.rs (если существует) + src/agent/loop_detection/service.rs
---
6. SkillCache (cache.rs)
Назначение: Управление кэшем на уровне сессии и диска
Структура:
- session_cache: HashMap<SkillName, LoadedSkill> - скиллы в текущей сессии
- embedding_cache: HashMap<SkillName, Vec<f32>> - кэш векторов на диске
- max_loaded_skills: Лимит одновременно загруженных скиллов
Источник идеи: Паттерн кэширования из src/storage.rs + управление памятью из src/agent/memory.rs
---
🔄 Поток данных
Фаза 1: Инициализация бота (startup)
main.rs
  ↓
AgentExecutor::new()
  ↓
SkillRegistry::new()
  ↓
SkillLoader::load_all_metadata("skills/")
  ↓ (парсинг frontmatter)
SkillMetadata[] (только метаданные, ~50-100 токенов на скилл)
  ↓
EmbeddingService::batch_generate()
  ↓ (если нет в .embeddings_cache/)
Mistral API → Vec<f32>[256] для каждого description
  ↓
Сохранить в .embeddings_cache/
Время: ~1-2 секунды при холодном старте, ~50ms при прогретом кэше
---
Фаза 2: Получение запроса от пользователя
User Message: "Найди видео про Rust и скачай его"
  ↓
AgentExecutor::execute_task()
  ↓
create_agent_system_prompt(user_message)
  ↓
┌─────────────────────────────────────┐
│ 1. Загрузить core.md (Always)      │
│    → ~500 токенов                   │
└─────────────────────────────────────┘
  ↓
┌─────────────────────────────────────┐
│ 2. SkillMatcher::select_skills()   │
│    Input: "Найди видео про Rust..." │
└─────────────────────────────────────┘
  ↓
2a. Быстрая проверка триггеров:
    - web-search.md: ["найди", "поищи"] ✓
    - video-processing.md: ["видео", "скачай"] ✓
  ↓
2b. Семантическая проверка (параллельно):
    - EmbeddingService::generate_embedding(user_message)
    - cosine_similarity с каждым скиллом
    - Топ-3 по score: 
      1. video-processing (0.87)
      2. web-search (0.72)
      3. file-management (0.45)
  ↓
2c. Объединение результатов:
    - video-processing: weight=High, trigger_match=true, semantic_score=0.87 → LOAD
    - web-search: weight=Medium, trigger_match=true, semantic_score=0.72 → LOAD
    - file-management: weight=Medium, semantic_score=0.45 → SKIP
  ↓
┌─────────────────────────────────────┐
│ 3. SkillRegistry::load_skills()    │
│    ["video-processing", "web-search"]│
└─────────────────────────────────────┘
  ↓
Загрузка контента:
  - video-processing.md → ~800 токенов
  - web-search.md → ~300 токенов
  ↓
┌─────────────────────────────────────┐
│ 4. Формирование System Prompt      │
│    core.md (500)                    │
│    + video-processing.md (800)      │
│    + web-search.md (300)            │
│    = 1600 токенов                   │
│    vs старый AGENT.md (2000 токенов)│
│    Экономия: 20%                    │
└─────────────────────────────────────┘
---
Фаза 3: Dynamic Loading (во время выполнения)
LLM Response: tool_call → ytdlp_download_video
  ↓
AgentExecutor::execute_tools()
  ↓
SkillRegistry::get_skill_by_tool("ytdlp_download_video")
  ↓
Проверка: video-processing.md уже загружен? → ДА
  ↓
SKIP (не добавляем дубликат)
---
LLM Response: tool_call → upload_file
  ↓
SkillRegistry::get_skill_by_tool("upload_file")
  ↓
Проверка: file-hosting.md загружен? → НЕТ
  ↓
load_skill_content("file-hosting")
  ↓
AgentMemory::add_message(system_message):
  "[Загружен модуль: file-hosting]
   ## Хостинг файлов
   - upload_file: загрузить файл в GoFile..."
  ↓
Следующая итерация LLM получает этот контекст
---
🔗 Интеграция с существующей архитектурой
Модификация AgentExecutor (executor.rs)
Текущая функция create_agent_system_prompt() (строка 1143):
- Читает AGENT.md целиком
- ~2000 токенов фиксированно
Новая логика:
1. Проверить наличие skills/ директории
2. Если есть → использовать Skill System
3. Если нет → fallback на старый AGENT.md
Точки интеграции:
- Добавить поле skill_registry: Arc<SkillRegistry> в AgentExecutor
- Модифицировать execute_tools() для dynamic loading
- Добавить метод refresh_skills_if_needed() для hot-reload в dev режиме
---
Модификация AgentSession (session.rs)
Добавить:
- loaded_skills: HashSet<String> - трекинг активных скиллов в сессии
- skill_token_count: usize - сколько токенов занимают скиллы
Методы:
- reset() → очистить loaded_skills
- get_active_skills() → список для debug UI
---
Модификация AgentMemory (memory.rs)
Опционально (для строгого трекинга):
- Добавить тип сообщения MessageRole::SkillContext
- При компактации памяти - сохранять skill context messages
Альтернатива: Использовать существующий MessageRole::System с префиксом [Skill: name]
---
Добавление Mistral Embedding Provider (llm/providers.rs)
Новый метод в LlmClient:
pub async fn generate_embedding(
    &self,
    text: &str,
    model: &str  // "mistral-embed"
) -> Result<Vec<f32>>
Endpoint: POST https://api.mistral.ai/v1/embeddings
Request:
{
  model: mistral-embed,
  input: [text to embed],
  encoding_format: float
}
Response: 256-размерный вектор
Источник идеи: Существующие методы chat_completion() в src/llm/providers.rs (строки 33-41, 222-230)
---
🚀 Миграционный путь
Этап 1: Инфраструктура (День 1-2)
Создать модули:
- src/agent/skills/types.rs - основные структуры
- src/agent/skills/loader.rs - парсинг YAML frontmatter
- src/agent/skills/mod.rs - публичный API
Идеи:
- YAML парсинг через serde_yaml (уже в зависимостях?)
- Markdown парсинг через pulldown-cmark или простой split по ---
- Паттерн структур из src/agent/memory.rs
Тест: Парсинг одного тестового файла skills/test.md
---
Этап 2: Embedding Integration (День 2-3)
Создать:
- src/agent/skills/embeddings.rs - Mistral API клиент
- src/agent/skills/cache.rs - сохранение векторов
Добавить в конфиг (src/config.rs):
pub const MISTRAL_EMBED_MODEL: &str = "mistral-embed";
pub const EMBEDDING_CACHE_DIR: &str = ".embeddings_cache/skills";
pub const EMBEDDING_DIMENSION: usize = 256;
Идеи:
- HTTP клиент из src/llm/http_utils.rs
- Сериализация векторов через bincode или serde_json
- Cosine similarity: стандартная формула dot(a,b) / (norm(a) * norm(b))
Тест: Генерация и сохранение embedding для одного скилла
---
Этап 3: Registry & Matching (День 3-4)
Создать:
- src/agent/skills/registry.rs - центральный менеджер
- src/agent/skills/matcher.rs - гибридное сопоставление
Идеи:
- Паттерн регистрации из src/agent/registry.rs
- Алгоритм матчинга:
  1. Keyword match: простой contains() по триггерам
  2. Semantic match: embedding similarity threshold (например, >0.6)
  3. Объединение: weighted score = 0.3 * keyword_match + 0.7 * semantic_score
Тест: Выбор скиллов для тестовых user messages
---
Этап 4: Интеграция с Executor (День 4-5)
Модифицировать:
- src/agent/executor.rs::create_agent_system_prompt() - использовать SkillRegistry
- src/agent/executor.rs::execute_tools() - dynamic loading
Логика:
1. При создании AgentExecutor → инициализировать SkillRegistry
2. В create_agent_system_prompt(user_message):
   - Загрузить skills/core.md (always)
   - Вызвать registry.select_skills(user_message, token_budget=1500)
   - Собрать контент выбранных скиллов
   - Вернуть объединённый промпт
Тест: E2E тест с реальным LLM запросом
---
Этап 5: Migration & Cleanup (День 5)
Разбить AGENT.md:
1. Скопировать текущий AGENT.md в AGENT.md.backup
2. Создать структуру skills/:
   - core.md - строки 1-10, 91-150 (общие правила)
   - web-search.md - строки 31-33
   - video-processing.md - строки 35-66
   - file-management.md - строки 14-24
   - file-hosting.md - строки 25-29
   - task-planning.md - строки 5-90
3. Добавить frontmatter в каждый файл:
      ---
   name: web-search
   description: Поиск и извлечение информации из интернета. Используй для актуальных данных, новостей, документации.
   triggers: [найди, поищи, поиск, актуальн, новост]
   allowed_tools: [web_search, web_extract]
   weight: medium
   ---
   
Тест: Сравнение качества ответов (старый vs новый промпт)
---
📊 Метрики и мониторинг
Debug UI (опционально)
Добавить в ответ агента (когда DEBUG=true):
✅ Задача выполнена
[Debug: Loaded skills]
- core (always, 500 tokens)
- video-processing (semantic: 0.87, 800 tokens)
- web-search (trigger: найди, 300 tokens)
Total skill tokens: 1600 / 2000 budget
Реализация: Добавить в AgentSession::get_debug_info()
---
Логирование
Добавить в tracing:
info!(
    skills = ?selected_skills,
    total_tokens = skill_token_count,
    savings_percent = ((2000 - skill_token_count) * 100) / 2000,
    "Skills loaded for request"
);
Место: В create_agent_system_prompt() после выбора скиллов
---
🔧 Конфигурация (config.rs)
Добавить константы:
// Skill System
pub const SKILLS_DIR: &str = "skills";
pub const SKILL_TOKEN_BUDGET: usize = 1500;  // Max токенов для скиллов
pub const SKILL_EMBEDDING_THRESHOLD: f32 = 0.6;  // Min similarity score
pub const SKILL_MAX_SELECTED: usize = 3;  // Max скиллов за раз
pub const SKILL_CACHE_TTL_SECS: u64 = 3600;  // Hot reload в dev
// Mistral Embeddings
pub const MISTRAL_EMBED_MODEL: &str = "mistral-embed";
pub const EMBEDDING_DIMENSION: usize = 256;
pub const EMBEDDING_CACHE_DIR: &str = ".embeddings_cache";
Environment Variables (для override):
SKILL_TOKEN_BUDGET=1500
SKILL_SEMANTIC_THRESHOLD=0.6
MISTRAL_EMBED_MODEL=mistral-embed
---
⚠️ Edge Cases и Обработка ошибок
1. Отсутствие skills/ директории
- Fallback на старый AGENT.md
- Warning в логах
2. Ошибка парсинга frontmatter
- Пропустить скилл с ошибкой
- Error в логах с указанием файла
- Продолжить с остальными скиллами
3. Mistral API недоступен
- Использовать только keyword matching (триггеры)
- Загрузить скиллы с weight=High по умолчанию
- Warning в логах
4. Превышение token budget
- Сортировать скиллы по priority: Always > High > Medium
- Загружать пока не исчерпан бюджет
- Логировать пропущенные скиллы
5. Некорректный embedding dimension
- Проверка после API вызова: vec.len() == 256
- Если нет - отклонить вектор, использовать keyword fallback
---
🎯 Критерии успеха
1. Экономия токенов: ≥50% для простых запросов
2. Точность активации: ≥90% правильно выбранных скиллов
3. Латентность: <100ms на выбор скиллов (после кэширования)
4. Backward compatibility: Работа без skills/ → fallback на AGENT.md
5. Developer UX: Hot reload скиллов в dev режиме
---
📝 Финальная структура проекта
Another-Chat-with-LLM/
├── skills/                     # [НОВОЕ] Модульные промпты
│   ├── core.md
│   ├── web-search.md
│   ├── video-processing.md
│   ├── file-management.md
│   ├── file-hosting.md
│   └── task-planning.md
├── .embeddings_cache/          # [НОВОЕ] Кэш векторов (gitignore)
│   └── skills/
│       ├── web-search.bin
│       └── ...
├── src/
│   ├── agent/
│   │   ├── skills/             # [НОВОЕ] Skill System
│   │   │   ├── mod.rs
│   │   │   ├── types.rs
│   │   │   ├── loader.rs
│   │   │   ├── registry.rs
│   │   │   ├── embeddings.rs
│   │   │   ├── matcher.rs
│   │   │   └── cache.rs
│   │   ├── executor.rs         # [МОДИФИЦИРОВАТЬ]
│   │   ├── session.rs          # [МОДИФИЦИРОВАТЬ]
│   │   └── memory.rs           # [ОПЦИОНАЛЬНО]
│   ├── llm/
│   │   ├── providers.rs        # [МОДИФИЦИРОВАТЬ] добавить embedding
│   │   └── mod.rs              # [МОДИФИЦИРОВАТЬ]
│   └── config.rs               # [МОДИФИЦИРОВАТЬ]
├── AGENT.md                    # [DEPRECATED] оставить как fallback
├── AGENT.md.backup             # [НОВОЕ] бэкап при миграции
└── Cargo.toml                  # [ПРОВЕРИТЬ] зависимости
---