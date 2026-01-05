# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Phase 2 - Preprocessor Fixes:
  - ✅ Исправлены 10 lints в `src/agent/preprocessor.rs`.
  - ✅ Inline переменных в format! (safe_name, mime, bytes, msg).
  - ✅ Рефакторинг `branches_sharing_code` (вынесен общего кода перед if let).
  - ✅ Добавлены `#[allow(clippy::cast_precision_loss)]` с обоснованием.
  - ✅ Числовые литералы в тестах приведены к стандарту с разделителями.
  - ✅ Пройдены все тесты (9/9).
- Phase 1 - Utils: `expect_used` Lints fixes
  - Исправлены `expect_used` lints в `src/utils.rs` для повышения надежности.
  - Добавлена зависимость `lazy-regex` для compile-time проверки регулярных выражений.
  - Добавлено 7 новых unit-тестов в `src/utils.rs` (общее количество тестов: 38/38).
- File upload support: users can now send documents to the agent in Telegram
- Files are automatically saved to `/workspace/uploads/` in the sandbox
- 1 GB total upload limit per session
- Smart file type hints for common formats (code, data, archives, images, etc.)
- File download support: agent can send files from sandbox to user via `send_file_to_user` tool
- 50 MB file size limit for downloads (Telegram API limit)
