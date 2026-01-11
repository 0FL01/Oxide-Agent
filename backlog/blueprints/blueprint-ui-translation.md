## Phase 1: Agent UI Components [ ]

**Goal**: Translate all static text responses and keyboard layouts in the Agent View layer to English.

**Resource Context**:
- 📄 `src/bot/views/agent.rs`
- 📚 **Docs**: `teloxide::types::KeyboardButton`

**Steps**:
1. [ ] **Implementation**: In `src/bot/views/agent.rs`:
    - Update `DefaultAgentView::welcome_message` to English.
    - Translate all status methods (`task_processing`, `task_cancelled`, etc.).
    - Translate error messages (`session_not_found`, `container_error`, etc.).
    - Update `loop_type_label` return values.
2. [ ] **Keyboards**: Update keyboard text definitions:
    - `get_agent_keyboard`: "❌ Cancel Task", "🗑 Clear Memory", "🔄 Recreate Container", "⬅️ Exit Agent Mode".
    - `loop_action_keyboard`: "Retry w/o detection", "Reset task", "Cancel".
    - `wipe_confirmation_keyboard`: "✅ Yes", "❌ Cancel".
3. [ ] **Verification**: Run `cargo-check` to ensure string literal syntax is correct.

## Phase 2: Agent Logic Synchronization [ ]

**Goal**: Update event handlers to match the English keyboard text and translate dynamic agent messages.

**Resource Context**:
- 📄 `src/bot/agent_handlers.rs`
- 📄 `src/bot/views/agent.rs` (Reference for button text)

**🛡 Invariant Check (Logic Sync)**:
- The string literals in `match text` blocks MUST exactly match the button labels defined in Phase 1.

**Steps**:
1. [ ] **Sync Handlers**: In `src/bot/agent_handlers.rs`, update `handle_agent_message` match arms:
    - Match "❌ Cancel Task" instead of Russian equivalent.
    - Match "🗑 Clear Memory".
    - Match "🔄 Recreate Container".
    - Match "⬅️ Exit Agent Mode".
2. [ ] **Sync Confirmation**: Update `handle_agent_wipe_confirmation` to match "✅ Yes" and "❌ Cancel".
3. [ ] **Dynamic Messages**: Translate internal status messages:
    - In `run_agent_task`: "⏳ Processing task...".
    - In `execute_agent_task`: Timeout error message.
    - In `send_loop_detected_message`: Loop detection header.
    - In `exit_agent_mode`: Exit confirmation message.
4. [ ] **QA**: Run `cargo-check` to verify code compiles.

## Phase 3: Main Bot Logic & Menus [ ]

**Goal**: Translate the main menu, chat mode, commands, and synchronize their handlers.

**Resource Context**:
- 📄 `src/bot/handlers.rs`

**Steps**:
1. [ ] **Commands**: Update `BotCommands` derive macros to use English descriptions for `/start`, `/clear`, `/healthcheck`, `/stats`.
2. [ ] **Keyboards**: Update functions to return English buttons:
    - `get_main_keyboard`: "🤖 Agent Mode", "💬 Chat Mode".
    - `get_chat_keyboard`: "Clear Context", "Change Model", "Extra Functions", "Back".
    - `get_extra_functions_keyboard`: "Edit Prompt", "Back".
    - `get_model_keyboard`: "Back" button.
3. [ ] **Sync Handlers**: In `handle_menu_commands`, update `match text` to correspond EXACTLY to the new button texts above.
4. [ ] **Responses**: Translate:
    - `start` function: Welcome message.
    - `stats` function: Anti-spam statistics message.
    - `clear` function: Success/Error messages.
    - `handle_voice`/`handle_photo`/`handle_document`: Error messages and status updates.
    - `check_agent_access`: Access denied messages.
5. [ ] **QA**: Run `cargo-check`.

## Phase 4: Narrator Localization [ ]

**Goal**: Configure the Narrator LLM to output status updates in English.

**Resource Context**:
- 📄 `src/agent/narrator.rs`

**Steps**:
1. [ ] **System Prompt**: In `Narrator::system_prompt()`:
    - Change "Use Russian language for output" to "Use English language for output".
    - Update the JSON example values (`headline`, `content`) to be in English (e.g., "Analyzing project structure").
2. [ ] **Final Verification**: Run `cargo-check` to ensure the project compiles successfully with all changes.
