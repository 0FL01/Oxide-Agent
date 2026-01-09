# Blueprint: Narrator System (Agent Informativeness Improvement)

**Status:** Planned  
**Date:** 2026-01-09  
**Goal:** Replace dry, technical execution logs with a narrative "Thinking/Action" style status update using a fast sidecar LLM. This aims to improve user experience by explaining *why* actions are taken, not just *what* they are.

## 1. Core Concept
Use a secondary, lightweight LLM (Sidecar) to interpret the primary agent's raw reasoning and tool calls into a concise, human-readable narrative.

**Target Model:** `labs-mistral-small-creative` (Provider: Mistral AI)  
**Constraints:** 
- Strict 1 RPS limit handling.
- Fallback to static templates if the narrator fails.
- Low latency requirement (should not block tool execution).

## 2. Architecture & Components

### A. Narrator Module (`src/agent/narrator.rs`)
A specialized wrapper around `LlmClient` dedicated to generating status updates.

*   **Input:** 
    *   `reasoning_content`: Raw CoT from the main agent.
    *   `tool_calls`: List of tools the agent intends to execute.
*   **Output (JSON):**
    ```json
    {
      "headline": "Short Action-Oriented Title (3-5 words)",
      "content": "2-3 sentences explaining the context and intent to the user."
    }
    ```
*   **Prompt Strategy:** 
    *   Role: Technical commentator/Narrator.
    *   Style: Professional, concise, action-oriented. "Thinking: ..." or "Action: ...".
*   **Error Handling:** Return `Option<Narrative>` to allow graceful fallback to existing static templates.

### B. Progress System Updates (`src/agent/progress.rs`)
Refactor the event system and UI rendering to support the narrative style.

*   **New Event:**
    ```rust
    AgentEvent::Narrative { 
        headline: String, 
        content: String 
    }
    ```
*   **State Updates (`ProgressState`):**
    *   Add `narrative_headline` and `narrative_content`.
    *   Deprecate direct usage of `current_thought` for display (keep for history/debugging).
*   **UI Rendering (`format_telegram`):**
    *   **Old:** "💭 Размышления: [Raw text] ... 🔧 execute_command"
    *   **New:**
        ```html
        <b>🧠 [Headline]</b>
        [Narrative Content]
        
        ⚡ <b>Action:</b> [Current Tool Name]
        
        📋 Tasks: ...
        ```

### C. Executor Integration (`src/agent/executor.rs`)
Inject the narrator call into the main execution loop (`run_loop`).

*   **Timing:** Immediately after receiving the response from the Main LLM.
*   **Concurrency:** Spawn the narrator task (`tokio::spawn`) so it runs in parallel with tool parameter preparation/sanitization, but aim to update the UI *before* the potentially long-running tools start, or update it asynchronously as soon as the narrative is ready.

### D. Configuration
*   **Env Var:** `NARRATOR_MODEL` (default: `mistral-small-latest`)
*   **Env Var:** `NARRATOR_PROVIDER` (default: `mistral`)

## 3. Implementation Steps

1.  **Scaffold Module:** Create `src/agent/narrator.rs` with structs and `LlmClient` integration.
2.  **Define Prompt:** Create the system prompt for the narrator in `src/agent/prompt/narrator.rs` (or inside the module).
3.  **Update Events:** Modify `AgentEvent` and `ProgressState` in `src/agent/progress.rs`.
4.  **Update UI:** Rewrite `ProgressState::format_telegram` to use the new fields.
5.  **Wire it up:** Call the narrator in `AgentExecutor::run_loop` and send the event.
