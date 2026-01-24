# Handover: Workspace Split + Transport Isolation

Дата: 2026-01-20
Ветка: `testing`

## Цель

Сделать универсальный рантайм агента, который можно подключать к разным транспортам (Telegram/Discord/Slack), и убрать любые зависимости/упоминания Telegram из ядра.

## Итоговая архитектура

Репозиторий переведен в Cargo workspace. Код разделен на 4 крейта:

- `crates/oxide-agent-core`
  - Ядро агента: выполнение, инструменты, LLM клиенты, storage/sandbox, доменные типы.
  - Не содержит `teloxide`.
  - Не содержит упоминаний "Telegram"/"telegram" в коде (включая тесты/строки/идентификаторы).

- `crates/oxide-agent-runtime`
  - Оркестрация и рантайм: транспорт-независимая обработка `AgentEvent`, прогресс-луп, реестр сессий.
  - Зависит от `oxide-agent-core`.
  - Не содержит `teloxide` и упоминаний Telegram.

- `crates/oxide-agent-transport-telegram`
  - Единственный слой, который знает про Telegram и тянет `teloxide`.
  - Вся логика Telegram UI/ретраев/рендера прогресса/отправки файлов живет здесь.

- `crates/oxide-agent-telegram-bot`
  - Бинарник (entrypoint). Поднимает настройки/логирование и запускает Telegram transport.

## Конфигурация

Сделан минимальный разрез конфигов:

- `AgentSettings` (в `crates/oxide-agent-core/src/config.rs`): все, что нужно агенту/LLM/tools/runtime/storage.
- `TelegramSettings` (в `crates/oxide-agent-transport-telegram/src/config.rs`): токен Telegram и списки доступа/параметры, специфичные для Telegram.

Цель разреза: core/runtime не должны требовать `telegram_token`.

## Что было критично исправлено ("хвосты")

После workspace split добит "нулевой хвост":

- Все упоминания `telegram`/`Telegram` удалены из `oxide-agent-core` и `oxide-agent-runtime`.
- Telegram-специфичные идентификаторы/функции переименованы в transport-neutral (пример: sandbox provider delivery).
- Telegram retry helper не должен жить в core/runtime; используется generic retry или переносится в transport.

## Как проверить

Команды:

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings

# "Zero tails" check
rg -n "(?i)telegram|teloxide" crates/oxide-agent-core crates/oxide-agent-runtime
```

Ожидания:

- `cargo test` green
- `cargo clippy` без warnings
- `rg` не находит совпадений

## Как добавить новый транспорт (Discord/Slack)

Рекомендуемая схема:

1) Создать новый crate: `crates/oxide-agent-transport-discord` или `crates/oxide-agent-transport-slack`.
2) Реализовать адаптер транспорта поверх публичного API runtime (progress loop + delivery).
3) Создать отдельный бинарник (по желанию): `crates/oxide-agent-discord-bot`.

Важно: новый транспорт не должен тянуть зависимости в core/runtime.

## Риски / Follow-ups

- Большой дифф (массовый перенос файлов). Перед любым merge рекомендовано делать отдельный review по структуре workspace и CI.
- Внутри core некоторые провайдеры все еще используют `i64` как session key на границе sandbox/storage. Это нормально, но если нужен межсервисный ключ (Discord+Telegram один пользователь) - стоит перейти на string/uuid `SessionKey` и аккуратно мигрировать storage keys.
- Если есть deployment scripts/инфра (Docker/CI), их нужно проверить на новый путь бинарника `crates/oxide-agent-telegram-bot`.
