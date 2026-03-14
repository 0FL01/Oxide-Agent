Oxide Agent - Handover Note
===================================

Дата: 14 марта 2026
Ветка: agent-topics
Что сделано: полная реализация multi-agent архитектуры с thread-based routing

Статус: IMPLEMENTED AND COMMITTED

===============================================================================
1. ОБЗОР ИЗМЕНЕНИЙ
===============================================================================

Реализована полная интеграция AGENT-TOPICS-BLUEPRINT.md:
- Stage 1: Telegram thread plumbing
- Stage 2: Per-topic конфигурация и роутинг
- Stage 3: Thread-aware session ключи для AgentMode
- Stage 4: Runtime dynamic thread bindings и topic lifecycle
- Stage 5: Manager control-plane с RBAC, audit и rollback

===============================================================================
2. НОВЫЕ ФУНКЦИИ
===============================================================================

2.1. THREAD-AWARE TELEGRAM ROUTING
------------------------------------------
- Ответы всегда уходят в правильный topic/thread
- Обработка edge case для General topic (id=1) - message_thread_id не включается
- Поддержка forum/dm/none типов потоков

Файлы: crates/oxide-agent-transport-telegram/src/bot/thread.rs

2.2. PER-TOPIC КОНФИГУРАЦИЯ
------------------------------------------
Возможность задавать настройки для каждого topic отдельно:

  enabled: включить/отключить бота в topic
  requireMention: требовать @mention для ответа
  agentId: назначить конкретного агента на topic
  skills: ограничить список доступных навыков
  systemPrompt: кастомный system prompt для topic

Файлы:
  - crates/oxide-agent-transport-telegram/src/config.rs
  - crates/oxide-agent-transport-telegram/src/bot/topic_route.rs

Конфигурация: config/local.yaml
  Пример структуры:
    telegram:
      topicConfigs:
        - chatId: -1001234567890
          threadId: 42
          agentId: "support-agent"
          requireMention: true
          enabled: true
          skills: ["faq", "billing"]

2.3. RUNTIME DYNAMIC BINDINGS
------------------------------------------
Агент может управлять привязками topic -> agent в runtime через manager tools:

  topic_binding_set: создать/обновить привязку
  topic_binding_get: посмотреть текущую привязку
  topic_binding_delete: удалить привязку
  topic_binding_rollback: откатить последнее изменение

Привязки поддерживают:
  binding_kind: manual | runtime
  chat_id: ID чата для изоляции
  thread_id: ID topic
  expires_at: срок действия привязки
  last_activity_at: трекинг активности

Приоритет роутинга: dynamic binding > static topic config > default

Файлы:
  - crates/oxide-agent-core/src/storage.rs (TopicBindingRecord, resolve_active_topic_binding)
  - crates/oxide-agent-core/src/agent/providers/manager_control_plane.rs

2.4. MANAGER CONTROL PLANE
------------------------------------------
Агент может управлять агентами и их профилями через manager tools:

  agent_profile_upsert: создать/обновить профиль агента
  agent_profile_get: получить профиль
  agent_profile_delete: удалить профиль
  agent_profile_rollback: откат изменений

Профиль агента включает:
  profileId: идентификатор профиля
  profileData: JSON объект с настройками
    modelId, modelProvider, modelName
    systemPrompt: кастомный system prompt
    systemPromptOverride: флаг переопределения

Все операции:
  - Сохраняются в R2 storage с версионированием
  - Записываются в audit trail
  - Поддерживают dry-run режим (preview без изменений)
  - Поддерживают rollback через audit history

Файлы:
  - crates/oxide-agent-core/src/storage.rs (AgentProfileRecord, AuditEventRecord)
  - crates/oxide-agent-core/src/agent/providers/manager_control_plane.rs

2.5. RBAC И БЕЗОПАСНОСТЬ
------------------------------------------
Права на управление через MANAGER_ALLOWED_USERS в .env

  - Только пользователи в списке могут использовать manager tools
  - Для manager-enabled executor внедряется ManagerControlPlaneProvider
  - Если пользователь не в списке, manager tools недоступны
  - Обычные пользователи работают с агентом в обычном режиме

Безопасность:
  - Process-local locks для RMW операций (StorageProvider)
  - Optimistic concurrency через ETag preconditions
  - SessionRegistry::remove_if_idle для безопасного удаления сессий
  - RBAC-aware session refresh без прерывания активных задач

Файлы:
  - crates/oxide-agent-transport-telegram/src/config.rs
  - crates/oxide-agent-transport-telegram/src/bot/agent_handlers.rs
  - crates/oxide-agent-core/src/storage.rs
  - crates/oxide-agent-runtime/src/session_registry.rs

2.6. THREAD-AWARE SESSION KEYS
------------------------------------------
Каждый topic получает изолированную сессию в AgentMode:

  Без thread: SessionId(user_id)
  С thread: SessionId(hash(user_id, chat_id, thread_id))

SessionRegistry API:
  - remove_if_idle: безопасное удаление только idle сессий
  - with_executor_mut: безопасная мутация без блокировки running tasks

Backward compatibility:
  - Primary: thread-aware ключ
  - Legacy: старый user-only ключ
  - Сначала пробуем primary, fallback на legacy если нужно
  - При смене RBAC refresh сессии с defer до завершения задачи

Файлы:
  - crates/oxide-agent-transport-telegram/src/bot/agent_handlers.rs
  - crates/oxide-agent-runtime/src/session_registry.rs

2.7. TELEGRAM FORUM TOPIC LIFECYCLE
------------------------------------------
Менеджер может управлять lifecycle forum topics из Agent Mode:

  forum_topic_create: создать новый topic
  forum_topic_edit: переименовать topic
  forum_topic_close: закрыть topic (архивировать)
  forum_topic_reopen: переоткрыть topic
  forum_topic_delete: удалить topic

Реализовано через абстракцию ManagerTopicLifecycle в core:
  - Transport-agnostic trait в oxide-agent-core
  - Telegram implementation в oxide-agent-transport-telegram
  - Инъекция в AgentExecutor только для manager-enabled пользователей

Файлы:
  - crates/oxide-agent-core/src/agent/providers/manager_control_plane.rs (trait, request models)
  - crates/oxide-agent-transport-telegram/src/bot/manager_topic_lifecycle.rs

2.8. AUDIT TRAIL
------------------------------------------
Все manager операции пишутся в audit log:

  Event types:
    - agent_profile_created
    - agent_profile_updated
    - agent_profile_deleted
    - topic_binding_created
    - topic_binding_updated
    - topic_binding_deleted
    - forum_topic_created
    - forum_topic_edited
    - forum_topic_closed
    - forum_topic_reopened
    - forum_topic_deleted

Metadata:
    - request: входные аргументы
    - result: результат операции
    - previous: предыдущее значение (для rollback)
    - audit_status: written | write_failed (non-fatal)

Storage:
  - keys: users/{user_id}/control_plane/audit/events.json
  - version: монотонное возрастание
  - pagination через list_audit_events_page для rollback scan

Файлы:
  - crates/oxide-agent-core/src/storage.rs
  - crates/oxide-agent-core/src/agent/providers/manager_control_plane.rs

===============================================================================
3. КОНФИГУРАЦИЯ
===============================================================================

3.1. ENVIRONMENT VARIABLES
------------------------------------------
TELEGRAM_TOKEN: токен бота
MANAGER_ALLOWED_USERS: список ID пользователей с доступом к управлению (через запятую)
AGENT_ACCESS_IDS: список ID пользователей с доступом к Agent Mode
R2_*: настройки Cloudflare R2 storage
RUN_MODE: режим конфигурации (development/production)

Новое переменные:
  MANAGER_ALLOWED_USERS: обязательная для активации manager control-plane

3.2. YAML КОНФИГУРАЦИЯ
------------------------------------------
Поддерживается иерархическая загрузка:

  1. config/default.yaml (базовые настройки, опционально)
  2. config/{RUN_MODE}.yaml (настройки для окружения, дефолт: config/development.yaml)
  3. config/local.yaml (локальные оверрайды, не коммитится)
  4. Переменные окружения (переопределяют файловые настройки)

Пример config/local.yaml:
  telegram:
    topicConfigs:
      - chatId: -1001234567890
        threadId: 42
        agentId: "support-agent"
        requireMention: true
        enabled: true
        skills: ["faq", "billing"]

3.3. DOCKER
------------------------------------------
Монтирование конфигурации:
  - ./config:/app/config (доступ к YAML файлам внутри контейнера)
  - /var/run/docker.sock (для песочницы)

Файл: docker-compose.yml

===============================================================================
4. СТРУКТУРА ПРОЕКТА
===============================================================================

4.1. КРАТЫ
------------------------------------------
oxide-agent-core:
  - storage.rs: StorageProvider trait + R2 impl + control-plane records + audit
  - agent/providers/manager_control_plane.rs: manager tools + topic lifecycle trait
  - config.rs: загрузка YAML конфигов
  - agent/executor.rs: AgentExecutor с injection manager control-plane и lifecycle
  - agent/providers/mod.rs: экспорты

oxide-agent-runtime:
  - session_registry.rs: реестр сессий + remove_if_idle API
  - agent/runtime/: оркестрация

oxide-agent-transport-telegram:
  - config.rs: TelegramSettings с manager_allowed_users
  - bot/handlers.rs: обработчики команд (chat mode)
  - bot/agent_handlers.rs: обработчики сообщений агенту (Agent Mode)
  - bot/topic_route.rs: роутинг с приоритетом dynamic > static
  - bot/thread.rs: thread extraction и outbound params
  - bot/manager_topic_lifecycle.rs: реализация ManagerTopicLifecycle
  - bot/agent_transport.rs: AgentTransport для песочницы

4.2. ТЕСТЫ
------------------------------------------
Unit tests:
  - crates/oxide-agent-transport-telegram/src/bot/thread.rs: thread helpers
  - crates/oxide-agent-transport-telegram/src/bot/topic_route.rs: routing resolution
  - crates/oxide-agent-core/src/agent/providers/manager_control_plane.rs: tools
  - crates/oxide-agent-runtime/src/session_registry.rs: registry API
  - crates/oxide-agent-core/src/storage.rs: storage operations

Integration tests:
  - crates/oxide-agent-transport-telegram/tests/topic_routing_thread_integration.rs
    - topic routing с override
    - thread replies
    - manager control-plane
    - session RBAC refresh
    - dynamic binding precedence

===============================================================================
5. КАК ЭТО РАБОТАЕТ
===============================================================================

5.1. ПОТОК СООБЩЕНИЯ
------------------------------------------
1. Входящее сообщение в Telegram
2. Telegram transport извлекает thread context (thread.rs)
3. resolve_topic_route определяет агент и настройки (topic_route.rs)
   - Сначала проверяется active dynamic binding (storage)
   - Если нет, используется static topic config
   - Если нет topic config, fallback на default
4. Проверяется enabled/requireMention
5. AgentExecutor обрабатывает сообщение
6. Outbound параметры строятся с thread context (thread.rs helpers)
7. Ответ отправляется в правильный topic/thread

5.2. ПОТОК УПРАВЛЕНИЯ
------------------------------------------
Менеджер в Agent Mode:

1. Агент получает команду от менеджера
2. Проверяется MANAGER_ALLOWED_USERS (RBAC)
3. Если доступ есть:
   - Выполняется manager tool:
     * topic_binding_set: создать/обновить привязку
     * agent_profile_upsert: создать профиль агента
     * forum_topic_create: создать topic
   - Изменения сохраняются в R2 storage с версией
   - Записывается audit event
4. Result возвращается LLM

Rollback:
1. Агент вызывает topic_binding_rollback или agent_profile_rollback
2. Provider сканирует audit events в обратном порядке
3. Находит последнее mutation событие
4. Восстанавливает previous snapshot
5. Записывает новый audit event

5.3. DRY-RUN РЕЖИМ
------------------------------------------
Для mutating manager tools:

1. Агент указывает dry_run=true
2. Provider:
   - Не выполняет storage mutation (upsert/delete)
   - Выполняет storage read для audit metadata
   - Возвращает preview результата с audit_status="preview"
3. Audit event пишется с outcome="dry_run"
4. Агент видит что произойдет, но изменения не применяются

5.4. SESSION REFRESH БЕЗОПАСНОСТЬ
------------------------------------------
При изменении MANAGER_ALLOWED_USERS:

1. Incoming message на существующей сессии
2. ensure_session_exists проверяет manager_control_plane_enabled()
3. Если статус изменился:
   - Проверяется remove_if_idle
   - Если сессия idle: удаляется и пересоздается с новым RBAC
   - Если сессия running: refresh откладывается до завершения задачи
4. После завершения задачи следующий ensure_session_exists пересоздаст сессию

===============================================================================
6. КЛЮЧЕВЫЕ ФАЙЛЫ
===============================================================================

6.1. КОНФИГУРАЦИЯ
------------------------------------------
.env.example: шаблон переменных окружения с MANAGER_ALLOWED_USERS
config/local.yaml: пример topic конфигурации

6.2. ДОКУМЕНТАЦИЯ
------------------------------------------
AGENTS.md: обновлено описание структуры проекта с новыми модулями

===============================================================================
7. ПРИМЕРЫ ИСПОЛЬЗОВАНИЯ
===============================================================================

7.1. СТАТИЧЕСКИЙ TOPIC ROUTING
------------------------------------------
В config/local.yaml:
  telegram:
    topicConfigs:
      - chatId: -1001234567890
        threadId: 42
        agentId: "support-agent"
        requireMention: true

Результат:
  - Сообщения в topic 42 всегда идут к support-agent
  - @mention обязателен для ответа

7.2. RUNTIME DYNAMIC BINDING
------------------------------------------
Команда менеджеру:
  "Создай тему для бага и назначь туда разработчика"

Агент выполняет:
  1. forum_topic_create(name="Баг #1234 - Ошибка оплаты")
  2. topic_binding_set(topic_id=156, agent_id="developer")

Результат:
  - Создан новый topic в Telegram
  - Создана привязка topic 156 -> developer
  - Все сообщения в этом topic идут агенту developer

7.3. СМЕНА АГЕНТА В EXISTING TOPIC
------------------------------------------
Команда менеджеру:
  "Переключи тему Поддержка на агента Sales"

Агент выполняет:
  topic_binding_set(topic_id=1, agent_id="sales")

Результат:
  - Смена происходит мгновенно (runtime binding)
  - Приоритет: dynamic binding > static config
  - Следующие сообщения в topic 1 идут агенту Sales

7.4. EXPIRY И ACTIVITY TRACKING
------------------------------------------
topic_binding_set с expires_at:
  topic_binding_set(topic_id=99, agent_id="coach", expires_at="2026-03-21T00:00:00Z")

Результат:
  - Через неделю привязка истекает
  - Роутинг fallback на static config или default
  - Activity timestamp обновляется при каждом сообщении в topic 99

7.5. ROLLBACK
------------------------------------------
Команда менеджеру:
  "Ой, я случайно назначил не того агента на тему Support"

Агент выполняет:
  topic_binding_rollback()

Результат:
  - Провайдер сканирует audit history
  - Находит последнее topic_binding_updated событие
  - Восстанавливает previous: agent_id="support-agent"
  - Записывает новый audit с event_type=topic_binding_rollback

===============================================================================
8. DEPENDENCIES И BUILD
===============================================================================

Rust 1.92
teloxide 0.17.0
AWS SDK (Cloudflare R2)
LLM providers: Groq, Mistral, ZAI, OpenRouter, Gemini

Build: cargo build
Test: cargo test

===============================================================================
9. POTENTIAL IMPROVEMENTS
===============================================================================

9.1. BACKGROUNG CLEANUP
------------------------------------------
CRON job для очистки expired bindings:
  - Сканировать bindings с expires_at < now
  - Удалять истекшие привязки
  - Оставлять audit запись о cleanup

9.2. TOPIC MONITORING
------------------------------------------
Бот может отправлять уведомления в manager topic:
  - Topic создан
  - Topic закрыт
  - Привязка истекает (за день до экспирации)

9.3. BATCH OPERATIONS
------------------------------------------
Bulk operations:
  - Обновление нескольких topic bindings за один раз
  - Создание нескольких topics одной командой (batch lifecycle tool)

===============================================================================
10. КОНТАКТЫ И ПОДДЕРЖКА
===============================================================================

Вопросы по реализации:
- См. AGENTS.md для описания модулей
- Ошибки хранятся в audit trail в R2 storage
- Logs: RUST_LOG=debug для отладки

===============================================================================
END OF HANDOVER NOTE
===============================================================================