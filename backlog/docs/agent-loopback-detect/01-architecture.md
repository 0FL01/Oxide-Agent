# Loop Detection Architecture

## Overview
Multi-layered infinite loop detection system for AI agents with three independent detection strategies.

## Components

### 1. LoopDetectionService
Central service coordinating all detection strategies.

**Responsibilities:**
- Track tool call repetitions
- Monitor streaming content for patterns
- Manage LLM-based periodic checks
- Coordinate detection events
- Handle session-level disable

### 2. Detection Strategies

#### Strategy A: Tool Call Loop Detection
- Target: Consecutive identical tool executions
- Threshold: 5 identical calls
- Hash-based comparison (SHA256)

#### Strategy B: Content Loop Detection
- Target: Repetitive text in streaming responses
- Algorithm: Sliding window with fixed-size chunks
- Threshold: 10 chunk repetitions within short distance
- Exclusions: Code blocks, tables, lists, headings

#### Strategy C: LLM-based Detection
- Target: Cognitive loops and unproductive conversation states
- Trigger: Periodic check (every 3-15 turns, after 30 turns min)
- Method: Dual-model verification (Flash → Pro)
- Threshold: 0.9 confidence

## Integration Points

```
Client Loop
├── Before each turn: turnStarted(signal) → LLM check
└── During streaming: addAndCheck(event) → Tool/Content check

Event Flow:
1. Detection triggers → LoopDetectedEvent
2. Abort current stream
3. Yield LoopDetected event to UI
4. UI offers retry with/without loop detection
```

## State Management

Per-session state:
- `promptId`: Current session identifier
- `disabledForSession`: Session-level flag
- `loopDetected`: Global detection flag
- Last tool call key + repetition count
- Content history buffer (max 5000 chars)
- Content chunk statistics (hash → positions)
- Turn counter for LLM checks

## Reset Conditions

- New prompt: `reset(promptId)`
- Different content elements detected
- Tool call boundary (resets content tracking)
- Session disable
