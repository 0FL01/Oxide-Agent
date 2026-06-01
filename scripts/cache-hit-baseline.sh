#!/usr/bin/env bash
# cache-hit-baseline.sh — DeepSeek V4 Flash prompt cache baseline via OpenCode Go.
#
# Measures prompt_cache_hit_tokens / prompt_cache_miss_tokens to establish
# a before/after baseline for the cache-hit optimization described in
# docs/tips/cache-hit.md.
#
# Two tests:
#   TEST 1 — STATIC PREFIX:  same system prompt, different user messages.
#            Simulates the *optimized* case (date at end, stable prefix).
#   TEST 2 — DYNAMIC PREFIX:  timestamp changes in first system message.
#            Simulates the *current* case (date at start, cache poisoned).
#
# Usage:
#   OPENCODE_GO_API_KEY=sk-... bash scripts/cache-hit-baseline.sh
#
# Requires: curl, jq.

set -euo pipefail

API_KEY="${OPENCODE_GO_API_KEY:?Set OPENCODE_GO_API_KEY}"
API_URL="https://opencode.ai/zen/go/v1/chat/completions"
MODEL="deepseek-v4-flash"

# ── helpers ──────────────────────────────────────────────────────────────

call_deepseek() {
    local payload="$1"
    curl -sS --max-time 120 \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "$payload" \
        "${API_URL}"
}

extract_usage() {
    local response="$1"
    echo "$response" | jq -r '
        .usage // {} |
        {
            prompt:            (.prompt_tokens // 0),
            cache_hit:         (.prompt_cache_hit_tokens // 0),
            cache_miss:        (.prompt_cache_miss_tokens // 0),
            completion:        (.completion_tokens // 0),
            total:             (.total_tokens // 0)
        }
    '
}

print_usage_row() {
    local label="$1" usage="$2" elapsed="$3"
    local p ch cm ct tt
    p=$(echo "$usage" | jq '.prompt')
    ch=$(echo "$usage" | jq '.cache_hit')
    cm=$(echo "$usage" | jq '.cache_miss')
    ct=$(echo "$usage" | jq '.completion')
    tt=$(echo "$usage" | jq '.total')

    local hit_rate="0.0"
    if [ "$p" -gt 0 ]; then
        # bc might not be available, use awk
        hit_rate=$(awk "BEGIN { printf \"%.1f\", ($ch / $p) * 100 }")
    fi

    printf "  %-22s  prompt=%5s  hit=%5s  miss=%5s  completion=%4s  hit_rate=%5s%%  %.1fs\n" \
        "$label" "$p" "$ch" "$cm" "$ct" "$hit_rate" "$elapsed"
}

# ── realistic system prompt (stable part ≈ 500 tokens) ───────────────────
#
# This mimics the size of workflow_guidance + structured_output blocks
# that Oxide Agent sends for a typical tool-enabled session.

read -r -d '' STABLE_SYSTEM << 'SYSTEM_EOF' || true
You are an AI assistant with access to tools. Follow these rules carefully.

## Core Behavior
- Always use available tools when they can help answer the user's request.
- Provide clear, concise responses.
- If you cannot complete a task, explain why and suggest alternatives.

## Workflow Hints

### When to use search tools
- Use web_search when the user asks about recent events, facts, or any information that may have changed.
- Use wiki_memory_read when the user refers to something discussed earlier in the conversation.

### When to use sandbox tools
- Use sandbox_exec when the user asks to run code, scripts, or commands.
- Use sandbox_write_file when the user wants to create or modify files.

### When to use reminder tools
- Use set_reminder when the user asks to be reminded about something at a specific time.
- Use list_reminders when the user wants to see their active reminders.

## Tool Access Policy
- Only use tools that are explicitly available.
- Never attempt to access tools not listed in your tool definitions.
- If a tool call fails, report the error and suggest alternative approaches.

## Structured Output
When your final answer requires structured data, output valid JSON.
All tool calls must use the exact parameter schema defined in the tool definition.
Do not invent parameters that are not in the schema.
SYSTEM_EOF

# ── TEST 1: STATIC PREFIX ───────────────────────────────────────────────

echo "============================================================"
echo "TEST 1: STATIC PREFIX (simulates date-at-end optimization)"
echo "  System prompt is identical across all 5 requests."
echo "  Expected: cache warming up, hit_rate rising."
echo "============================================================"
echo ""

for i in 1 2 3 4 5; do
    user_msg="This is user message number ${i}. What tools do you have available? Briefly list them."

    payload=$(jq -n \
        --arg model "$MODEL" \
        --arg system "$STABLE_SYSTEM" \
        --arg user "$user_msg" \
        '{
            model: $model,
            max_tokens: 256,
            temperature: 0.3,
            stream: false,
            messages: [
                { role: "system", content: $system },
                { role: "user",   content: $user }
            ]
        }')

    t_start=$(date +%s%N)
    response=$(call_deepseek "$payload")
    t_end=$(date +%s%N)
    elapsed=$(awk "BEGIN { printf \"%.1f\", ($t_end - $t_start) / 1e9 }")

    # Check for errors
    if echo "$response" | jq -e '.error' >/dev/null 2>&1; then
        echo "  Request $i ERROR: $(echo "$response" | jq -r '.error.message // .error')"
        continue
    fi

    usage=$(extract_usage "$response")
    print_usage_row "static-req-${i}" "$usage" "$elapsed"
done

# ── pause for cache persistence ──────────────────────────────────────────

echo ""
echo "  (pausing 5s for cache persistence...)"
sleep 5

# ── TEST 2: DYNAMIC PREFIX (timestamp in first line) ────────────────────

echo ""
echo "============================================================"
echo "TEST 2: DYNAMIC PREFIX (simulates current date-at-start bug)"
echo "  System prompt has a unique timestamp in the FIRST line."
echo "  Expected: cache miss on every request, hit_rate ≈ 0%."
echo "============================================================"
echo ""

for i in 1 2 3 4 5; do
    ts=$(date '+%Y-%m-%d %H:%M:%S')
    user_msg="This is user message number ${i}. What tools do you have available? Briefly list them."

    payload=$(jq -n \
        --arg model "$MODEL" \
        --arg ts "$ts" \
        --arg system "$STABLE_SYSTEM" \
        --arg user "$user_msg" \
        '{
            model: $model,
            max_tokens: 256,
            temperature: 0.3,
            stream: false,
            messages: [
                { role: "system", content: ("### CURRENT DATE AND TIME\nToday: " + $ts + "\n\n" + $system) },
                { role: "user",   content: $user }
            ]
        }')

    t_start=$(date +%s%N)
    response=$(call_deepseek "$payload")
    t_end=$(date +%s%N)
    elapsed=$(awk "BEGIN { printf \"%.1f\", ($t_end - $t_start) / 1e9 }")

    if echo "$response" | jq -e '.error' >/dev/null 2>&1; then
        echo "  Request $i ERROR: $(echo "$response" | jq -r '.error.message // .error')"
        continue
    fi

    usage=$(extract_usage "$response")
    print_usage_row "dynamic-req-${i}" "$usage" "$elapsed"
done

echo ""
echo "============================================================"
echo "Done. Compare hit_rate between TEST 1 and TEST 2."
echo "============================================================"
