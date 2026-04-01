# Gemini SDK Migration

Этот документ фиксирует Stage 0 для миграции Gemini provider в Oxide.

## Решение

- Интеграция Gemini переводится на `gemini-rust` как единственный SDK path.
- Ручной REST path не развивается и будет удален на cleanup stage.
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
- Дальнейшие stages должны опираться только на этот vendored path

## Acceptance Criteria для Stage 0/1

- Решение зафиксировано в репозитории
- Vendored `gemini-rust` содержит поддержку function call correlation ids
- Vendored `gemini-rust` умеет сериализовать и десериализовать `allowed_function_names`
- Подключение vendored crate зафиксировано в root `Cargo.toml`
