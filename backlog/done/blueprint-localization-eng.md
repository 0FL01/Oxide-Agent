## Phase 1: Core System Prompts & Identity [x]

**Goal**: Translate the agent's internal system prompts, date formatting, and fallback instructions to English to establish a native English persona.

**Resource Context**:
- 📄 `src/agent/prompt/composer.rs`
- 📄 `AGENTS.md`

**Steps**:
1. [x] **Update Composer Logic**: In `src/agent/prompt/composer.rs`:
    - Update `build_date_context` to use English day names (e.g., "Monday" remains "Monday", remove Cyrillic mapping).
    - Update `### CURRENT DATE AND TIME` header and instructions in `build_date_context`.
    - Translate the entire string in `get_fallback_prompt`.
    - Translate `build_structured_output_instructions` (JSON schema rules, "Ты ДОЛЖЕН" -> "You MUST").
    - Translate `create_sub_agent_system_prompt` ("Ты - легковесный суб-агент..." -> "You are a lightweight sub-agent...").
2. [x] **Validation**: Run `cargo-check` to ensure string literals are correctly terminated and formatted.

## Phase 2: Input Preprocessing & Vision [x]

**Goal**: Translate user-facing messages related to file uploads, image analysis, and file type hints.

**Resource Context**:
- 📄 `src/agent/preprocessor.rs`

**Steps**:
1. [x] **Vision Prompts**: In `describe_image`:
    - Translate the `system_prompt` for the vision model ("Ты - визуальный анализатор..." -> "You are a visual analyzer...").
    - Translate the user context wrapper ("Опиши это изображение..." -> "Describe this image in detail...").
2. [x] **File Processing**: In `process_document`:
    - Translate upload limit errors ("Превышен лимит загрузки..." -> "Upload limit exceeded...").
    - Translate the file info block ("📎 **Пользователь загрузил файл:**" -> "📎 **User uploaded a file:**", "Размер", "Тип", "Сообщение").
3. [x] **Type Hints**: In `get_file_type_hint`:
    - Translate all match arms (e.g., "💡 Исходный код..." -> "💡 Source code...", "💡 Архив..." -> "💡 Archive...").
4. [x] **Validation**: Run `cargo-check` to verify changes.

## Phase 3: Error Handling & Runner Feedback [x]

**Goal**: Translate system injection messages used for error recovery (JSON repair) and tool status updates.

**Resource Context**:
- 📄 `src/agent/runner/responses.rs`
- 📄 `src/agent/tool_bridge.rs`
- 📄 `src/agent/providers/filehoster.rs`
- 📄 `src/agent/hooks/completion.rs`

**Steps**:
1. [x] **JSON Repair**: In `src/agent/runner/responses.rs`, translate the `handle_structured_output_error` system message (" [СИСТЕМА: Ваш предыдущий ответ..." -> "[SYSTEM: Your previous response...").
2. [x] **Tool Errors**: In `src/agent/tool_bridge.rs`:
    - Translate cancellation messages ("Задача отменена пользователем" -> "Task cancelled by user").
    - Translate timeout messages.
3. [x] **File Hoster**: In `src/agent/providers/filehoster.rs`, translate upload error messages ("❌ Ошибка загрузки..." -> "❌ Upload error...").
4. [x] **Completion Hooks**: In `src/agent/hooks/completion.rs`, translate the task completion check message.
5. [x] **Validation**: Run `cargo-check`.

## Phase 4: Skills & Tests [x]

**Goal**: Ensure external skill definitions and tests use English to avoid confusion during execution.

**Resource Context**:
- 📄 `skills/*.md`
- 📄 `tests/sub_agent_delegation.rs`

**Steps**:
1. [x] **Skills Review**: Check `skills/` directory. If any markdown files contain Russian descriptions or triggers, translate them. (Note: `core.md` and `web-search.md` appear to be English already).
2. [x] **Test Data**: Update `tests/sub_agent_delegation.rs` to use English prompts in the `task` field.
3. [x] **Final Check**: Run `cargo-test --test sub_agent_delegation` to ensure the translation didn't break logic.
