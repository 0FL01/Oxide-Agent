# Gemini SDK Migration

Этот документ фиксирует итоговое решение и финальное состояние миграции Gemini provider в Oxide.

## Решение

- Интеграция Gemini переведена на `gemini-rust` как единственный SDK path.
- Старый ручной REST path удален из `GeminiProvider`.
- Для tool calling используется patched/vendored `gemini-rust 1.7.1` без fallback на старую реализацию.

## Почему нужен патч SDK

Актуальная Gemini function-calling схема требует provider correlation id:

- входящий `functionCall.id`
- исходящий `functionResponse.id`

Oxide должен сохранить этот id во внутренней корреляции tool call и вернуть тот же id при replay tool result. Без этого нельзя честно считать Gemini tool loop production-safe, особенно при нескольких вызовах одной функции в одном ходе.

## Обязательные расширения vendored SDK

- `FunctionCall { id: Option<String>, ... }`
- `FunctionResponse { id: Option<String>, ... }`
- `FunctionCallingConfig.allowed_function_names: Option<Vec<String>>`

## Принятый способ подключения

- Patched crate хранится в репозитории: `vendor/gemini-rust`
- Workspace подключает его через `[patch.crates-io]`
- Реализация Gemini в workspace опирается только на этот vendored path

## Итоговое состояние

- `GeminiProvider` работает через vendored `gemini-rust`, а не через ручной `generateContent` REST-код.
- Tool calling включен на уровне capability gating и runtime path.
- Structured JSON path поддерживается через `chat_with_tools(..., json_mode = true)` без tools.
- Tool-call correlation id проходит весь цикл:
  - Gemini `functionCall.id`
  - внутренняя корреляция Oxide
  - Gemini `functionResponse.id`
- История assistant/tool сообщений replay'ится в Gemini-совместимые `Role::Model` и `Role::User` сообщения с `FunctionCall` / `FunctionResponse` parts.

## Acceptance Criteria

- Решение зафиксировано в репозитории
- Vendored `gemini-rust` содержит поддержку function call correlation ids
- Vendored `gemini-rust` умеет сериализовать и десериализовать `allowed_function_names`
- Подключение vendored crate зафиксировано в root `Cargo.toml`
- Старый Gemini REST path удален из `crates/oxide-agent-core/src/llm/providers/gemini.rs`
- Gemini capability metadata отражает реальное состояние: tool loop и structured output доступны
- Regression-тесты покрывают correlation replay, structured response parsing, safety defaults и error mapping

## Cleanup Audit

- В core Gemini provider больше нет прямой сборки URL вида `...:generateContent?key=...`.
- Больше нет query-string auth для Gemini; SDK использует `x-goog-api-key`.
- Старые helper'ы `send_json_request` / `extract_text_content` не используются Gemini provider'ом.
