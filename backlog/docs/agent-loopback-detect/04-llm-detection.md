# LLM-based Loop Detection

## Parameters

```rust
const LLM_CHECK_AFTER_TURNS: usize = 30;
const DEFAULT_LLM_CHECK_INTERVAL: usize = 3;
const MIN_LLM_CHECK_INTERVAL: usize = 5;
const MAX_LLM_CHECK_INTERVAL: usize = 15;
const LLM_CONFIDENCE_THRESHOLD: f64 = 0.9;
const LLM_LOOP_CHECK_HISTORY_COUNT: usize = 20;
```

## Algorithm Overview

```
Turn 1-29: No checks
Turn 30: First check (every 3 turns initially)
Turn 33: Check (adapt interval based on confidence)
Turn 36+: Check at adaptive interval (5-15)
```

## State Management

```rust
struct LlmLoopTracker {
    turns_in_prompt: usize,
    check_interval: usize,
    last_check_turn: usize,
}
```

## Check Trigger Logic

```rust
async fn turn_started(&mut self, signal: AbortSignal) -> bool {
    self.turns_in_prompt += 1;

    let should_check = self.turns_in_prompt >= LLM_CHECK_AFTER_TURNS &&
                     (self.turns_in_prompt - self.last_check_turn) >= self.check_interval;

    if should_check {
        self.last_check_turn = self.turns_in_prompt;
        return self.check_for_loop_with_llm(signal).await;
    }

    false
}
```

## Dual-Model Verification

### Phase 1: Flash Model (Fast Scan)

```rust
async fn check_for_loop_with_llm(&mut self, signal: AbortSignal) -> bool {
    let history = self.get_recent_history(20);
    let flash_result = self.query_model("flash-model", &history, signal).await?;

    let flash_confidence = flash_result.confidence;

    if flash_confidence < LLM_CONFIDENCE_THRESHOLD {
        self.update_check_interval(flash_confidence);
        return false;
    }

    // Phase 2: Pro model confirmation
    let pro_result = self.query_model("pro-model", &history, signal).await?;
    let pro_confidence = pro_result.confidence;

    if pro_confidence >= LLM_CONFIDENCE_THRESHOLD {
        return true;  // Loop confirmed
    }

    self.update_check_interval(pro_confidence);
    false
}
```

### Phase 2: Pro Model (Confirmation)

- Only runs if Flash model is confident (≥ 0.9)
- Uses same conversation history
- Must also be confident (≥ 0.9)
- If Pro model unavailable, trust Flash model

## System Prompt

```
You are a sophisticated AI diagnostic agent specializing in identifying when a conversational AI is stuck in an unproductive state. Your task is to analyze the provided conversation history and determine if the assistant has ceased to make meaningful progress.

An unproductive state is characterized by one or more of the following patterns over the last 5 or more assistant turns:

Repetitive Actions: The assistant repeats the same tool calls or conversational responses a decent number of times. This includes simple loops (e.g., tool_A, tool_A, tool_A) and alternating patterns (e.g., tool_A, tool_B, tool_A, tool_B, ...).

Cognitive Loop: The assistant seems unable to determine the next logical step. It might express confusion, repeatedly ask the same questions, or generate responses that don't logically follow from the previous turns, indicating it's stuck and not advancing the task.

Crucially, differentiate between a true unproductive state and legitimate, incremental progress.
For example, a series of 'tool_A' or 'tool_B' tool calls that make small, distinct changes to the same file (like adding docstrings to functions one by one) is considered forward progress and is NOT a loop. A loop would be repeatedly replacing the same text with the same content, or cycling between a small set of files with no net change.
```

## JSON Schema for LLM Response

```json
{
  "type": "object",
  "properties": {
    "unproductive_state_analysis": {
      "type": "string",
      "description": "Your reasoning on if the conversation is looping without forward progress."
    },
    "unproductive_state_confidence": {
      "type": "number",
      "description": "A number between 0.0 and 1.0 representing your confidence that the conversation is in an unproductive state."
    }
  },
  "required": ["unproductive_state_analysis", "unproductive_state_confidence"]
}
```

## Adaptive Check Interval

```rust
fn update_check_interval(&mut self, confidence: f64) {
    self.check_interval = (MIN_LLM_CHECK_INTERVAL as f64 +
        (MAX_LLM_CHECK_INTERVAL - MIN_LLM_CHECK_INTERVAL) as f64 * (1.0 - confidence))
        .round() as usize;
}
```

**Formula:**

- High confidence (0.9) → Check frequently (every 5 turns)
- Low confidence (0.0) → Check rarely (every 15 turns)

## History Preparation

```rust
fn prepare_history_for_llm(&self) -> Vec<ConversationTurn> {
    let mut recent = self.history.clone();
    recent.truncate(LLM_LOOP_CHECK_HISTORY_COUNT);

    // Trim dangling function calls from end
    while let Some(last) = recent.last() {
        if last.is_function_call() {
            recent.pop();
        } else {
            break;
        }
    }

    // Trim leading function responses
    while let Some(first) = recent.first() {
        if first.is_function_response() {
            recent.remove(0);
        } else {
            break;
        }
    }

    recent
}
```

## Rust Implementation Sketch

```rust
pub struct LlmLoopDetector {
    turns_in_prompt: usize,
    check_interval: usize,
    last_check_turn: usize,
    flash_model: String,
    pro_model: String,
}

impl LlmLoopDetector {
    pub async fn check(&mut self, history: &[Message], signal: AbortSignal)
        -> Result<bool, Error>
    {
        let flash_result = self.query_model(
            &self.flash_model,
            &self.prepare_history(history),
            signal.clone()
        ).await?;

        if flash_result.confidence < LLM_CONFIDENCE_THRESHOLD {
            self.adapt_interval(flash_result.confidence);
            return Ok(false);
        }

        let pro_result = self.query_model(
            &self.pro_model,
            &self.prepare_history(history),
            signal
        ).await?;

        let detected = pro_result.confidence >= LLM_CONFIDENCE_THRESHOLD;
        self.adapt_interval(pro_result.confidence);
        Ok(detected)
    }
}
```

## Testing Scenarios

1. High confidence from both models: Detection
2. Low confidence from Flash: No detection
3. Flash high, Pro low: No detection
4. Flash high, Pro unavailable: Detection (trust Flash)
5. Confidence at exact threshold (0.9): Detection
