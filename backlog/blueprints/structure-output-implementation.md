# Blueprint: Structured Output (Full JSON Loop) Implementation

**Feature:** Structured Output / JSON Mode
**Target System:** Oxide Agent (Zai Provider & Executor)
**Reference Docs:** [@backlog/docs/zai/structure-output.md](../docs/zai/structure-output.md)

## Abstract

The goal is to transition the agent's communication protocol from a mix of free-text and native tool calls (which suffers from XML leakage and malformed outputs) to a **Strict Full JSON Loop**. The model will be instructed to output *only* valid JSON objects conforming to a specific schema.

## Data Structures

### Agent Output Schema (JSON)
The model must strictly adhere to this structure.

```json
{
  "thought": "Reasoning chain describing the thought process and decision making",
  "tool_call": {
    "name": "tool_name",
    "arguments": {
      "arg1": "value1",
      "arg2": "value2"
    }
  },
  "final_answer": "Final response text to the user (nullable if tool_call is present)"
}
```

**Constraints:**
- `tool_call` and `final_answer` are mutually exclusive (or logical flow dictates one active per step).
- If `tool_call` is present, `final_answer` should be null.
- If `final_answer` is present, `tool_call` should be null.

## Implementation Steps

### 1. LLM Provider Update (`src/llm/providers.rs`)
**Target:** `ZaiProvider` (and potentially others via trait update).

- Modify `chat_with_tools` (or create a new method) to:
  - Inject `"response_format": { "type": "json_object" }` into the API request body.
  - **Critical:** Ensure `stream=true` is handled correctly with JSON accumulation, OR switch to non-streaming for the control loop if streaming JSON parsing is too complex (though streaming is preferred for UX).
  - *Note:* If sticking to "Full JSON Loop" via prompt instructions, we might strictly disable native `tools` parameter in the API call to avoid ambiguity, or keep them but force JSON output. The "Full JSON Loop" usually implies defining tools in the system prompt schema.

### 2. Prompt Engineering (`src/agent/prompt/composer.rs`)
**Target:** `create_agent_system_prompt`

- **Remove:** Native tool definitions (if we are simulating them) or Instructions that allow free text.
- **Add:** Strict JSON System Instruction.
  - Define the `Agent Output Schema`.
  - Inject the available tools list *into the system prompt text* formatted as a JSON schema or a clear list of definitions.
  - Explicit instruction: "You must output ONLY valid JSON. No markdown blocks, no xml."

### 3. Agent Executor Refactoring (`src/agent/executor.rs`)
**Target:** `AgentExecutor::run_loop` & `preprocess_llm_response`

- **Parsing Logic:**
  - Replace the text/XML scraping logic (`recovery.rs`) with `serde_json::from_str::<AgentResponse>`.
  - Handle JSON parsing errors by feeding the error back to the model (Self-Correction).
- **Execution Flow:**
  - If `parsed.tool_call` is set -> Execute tool -> Add result to history.
  - If `parsed.final_answer` is set -> Finish task -> Return answer.

## Advantages
- **Determinism:** Eliminates "hallucinated" XML tags or mixed content.
- **Parsing:** `serde_json` is stricter and faster than regex/fuzzy matching.
- **Stability:** Prevents the "infinite loop" caused by malformed tool calls.

## Risks & Mitigation
- **JSON Syntax Errors:** The model might generate invalid JSON (e.g., unescaped quotes).
  - *Mitigation:* Use a "Retry with Error" loop. If parsing fails, send the error back to the model asking it to fix the JSON.
- **Streaming UX:** Streaming a full JSON object is harder to display progressively than text.
  - *Mitigation:* Stream the `thought` field to the UI as "Thinking...", then execute the tool.

---
*Signed: Oxide Architecture Team*
