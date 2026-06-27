#!/usr/bin/env bash
# Phase 0: Live-contract probe — verify providers accept tools[] growth across turns.
#
# Tests the fundamental contract: can tools[] differ between requests in the same conversation?
# Turn 1: tools=[retrieve_tools] → model calls retrieve_tools
# Turn 2: tools=[retrieve_tools, read_file] + history(tool_call + tool_result) → must succeed
#
# П0.5: Reality is verified BEFORE design stands on it.

set -euo pipefail

# Load .env
if [[ -f .env ]]; then
  set -a
  source .env
  set +a
fi

API_BASE="${OPENCODE_GO_API_BASE:-https://opencode.ai/zen/go/v1/chat/completions}"
API_KEY="${OPENCODE_GO_API_KEY:-}"
MODEL="${OPENCODE_GO_PROBE_MODEL:-deepseek-v4-flash}"

if [[ -z "$API_KEY" || "$API_KEY" == *"YOUR_"* ]]; then
  echo "FAIL: OPENCODE_GO_API_KEY not set or placeholder"
  exit 1
fi

echo "=== Phase 0: Live-contract probe ==="
echo "Provider: OpenCode Go (OpenAI chat/completions format)"
echo "Endpoint: $API_BASE"
echo "Model:    $MODEL"
echo ""

# retrieve_tools schema (minimal, matches our implementation)
RETRIEVE_TOOLS_TOOL='{
  "type": "function",
  "function": {
    "name": "retrieve_tools",
    "description": "Retrieve tools for a capability group. Call this to load additional tools before using them.",
    "parameters": {
      "type": "object",
      "properties": {
        "capabilities": {
          "type": "array",
          "items": {
            "type": "string",
            "enum": ["files","shell","web","browser","memory","media","ytdlp","tts","delegation","agents_md","manager","ssh","stack_logs","reminders","jira","mattermost"]
          },
          "description": "Capability groups to activate"
        },
        "reason": {"type": "string", "description": "Why these capabilities are needed"}
      },
      "required": ["capabilities"],
      "additionalProperties": false
    }
  }
}'

# read_file schema (a real tool that would be activated by retrieve_tools)
READ_FILE_TOOL='{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "Read the contents of a file from the sandbox filesystem.",
    "parameters": {
      "type": "object",
      "properties": {
        "path": {"type": "string", "description": "Absolute path to the file to read."}
      },
      "required": ["path"],
      "additionalProperties": false
    }
  }
}'

# --- Turn 1: Only retrieve_tools in tools[] ---
echo "--- Turn 1: tools=[retrieve_tools] ---"
TURN1_PAYLOAD=$(jq -n \
  --argjson tool "$RETRIEVE_TOOLS_TOOL" \
  '{
    model: "deepseek-v4-flash",
    messages: [
      {role: "system", content: "You are a coding assistant. You have limited tools available. If you need to read files, first call retrieve_tools with capabilities=[\"files\"] to load file tools."},
      {role: "user", content: "I need you to read /tmp/test.txt. Please retrieve the necessary tools first."}
    ],
    tools: [$tool],
    tool_choice: "auto",
    max_tokens: 512,
    stream: false
  }')

TURN1_RESPONSE=$(curl -s -w "\n%{http_code}" \
  -X POST "$API_BASE" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $API_KEY" \
  -d "$TURN1_PAYLOAD")

TURN1_HTTP_CODE=$(echo "$TURN1_RESPONSE" | tail -1)
TURN1_BODY=$(echo "$TURN1_RESPONSE" | sed '$d')

echo "HTTP status: $TURN1_HTTP_CODE"

if [[ "$TURN1_HTTP_CODE" != "200" ]]; then
  echo "FAIL: Turn 1 returned HTTP $TURN1_HTTP_CODE"
  echo "Response: $TURN1_BODY" | head -20
  exit 1
fi

# Check for tool_calls in response
TURN1_TOOL_CALLS=$(echo "$TURN1_BODY" | jq -r '.choices[0].message.tool_calls // empty')
if [[ -z "$TURN1_TOOL_CALLS" || "$TURN1_TOOL_CALLS" == "null" ]]; then
  echo "WARN: Turn 1 did not produce tool_calls. Model response:"
  echo "$TURN1_BODY" | jq -r '.choices[0].message.content // "no content"' | head -5
  echo "This is acceptable for the probe — we can still test turn 2 with synthetic history."
  TOOL_CALL_ID="call_probe_001"
  TOOL_CALL_ARGS='{"capabilities": ["files"]}'
else
  TOOL_CALL_ID=$(echo "$TURN1_BODY" | jq -r '.choices[0].message.tool_calls[0].id')
  TOOL_CALL_ARGS=$(echo "$TURN1_BODY" | jq -r '.choices[0].message.tool_calls[0].function.arguments')
  echo "Model called: $(echo "$TURN1_BODY" | jq -r '.choices[0].message.tool_calls[0].function.name')($TOOL_CALL_ARGS)"
  echo "Tool call id: $TOOL_CALL_ID"
fi

echo ""

# --- Turn 2: EXPANDED tools[] + history with tool_call/tool_result ---
echo "--- Turn 2: tools=[retrieve_tools, read_file] + history ---"

# Build the tool result (simulating what retrieve_tools would return)
TOOL_RESULT='{"activated":[{"name":"read_file","group":"files"},{"name":"write_file","group":"files"},{"name":"apply_file_edit","group":"files"},{"name":"list_files","group":"files"}],"already_active":[],"unknown_groups":[]}'

TURN2_PAYLOAD=$(jq -n \
  --argjson retrieve_tool "$RETRIEVE_TOOLS_TOOL" \
  --argjson read_file_tool "$READ_FILE_TOOL" \
  --arg tool_call_id "$TOOL_CALL_ID" \
  --arg tool_call_args "$TOOL_CALL_ARGS" \
  --arg tool_result "$TOOL_RESULT" \
  '{
    model: "deepseek-v4-flash",
    messages: [
      {role: "system", content: "You are a coding assistant. You have limited tools available. If you need to read files, first call retrieve_tools with capabilities=[\"files\"] to load file tools."},
      {role: "user", content: "I need you to read /tmp/test.txt. Please retrieve the necessary tools first."},
      {
        role: "assistant",
        content: null,
        tool_calls: [{
          id: $tool_call_id,
          type: "function",
          function: {name: "retrieve_tools", arguments: $tool_call_args}
        }]
      },
      {
        role: "tool",
        tool_call_id: $tool_call_id,
        content: $tool_result
      },
      {role: "user", content: "Now read /tmp/test.txt"}
    ],
    tools: [$retrieve_tool, $read_file_tool],
    tool_choice: "auto",
    max_tokens: 512,
    stream: false
  }')

TURN2_RESPONSE=$(curl -s -w "\n%{http_code}" \
  -X POST "$API_BASE" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $API_KEY" \
  -d "$TURN2_PAYLOAD")

TURN2_HTTP_CODE=$(echo "$TURN2_RESPONSE" | tail -1)
TURN2_BODY=$(echo "$TURN2_RESPONSE" | sed '$d')

echo "HTTP status: $TURN2_HTTP_CODE"

if [[ "$TURN2_HTTP_CODE" != "200" ]]; then
  echo "FAIL: Turn 2 returned HTTP $TURN2_HTTP_CODE"
  echo "Response body:"
  echo "$TURN2_BODY" | jq . 2>/dev/null || echo "$TURN2_BODY" | head -20
  exit 1
fi

# Check what the model did with the expanded tool set
TURN2_TOOL_CALLS=$(echo "$TURN2_BODY" | jq -r '.choices[0].message.tool_calls // empty')
if [[ -n "$TURN2_TOOL_CALLS" && "$TURN2_TOOL_CALLS" != "null" ]]; then
  TURN2_TOOL_NAME=$(echo "$TURN2_BODY" | jq -r '.choices[0].message.tool_calls[0].function.name')
  echo "Model called: $TURN2_TOOL_NAME"
  if [[ "$TURN2_TOOL_NAME" == "read_file" ]]; then
    echo "PASS: Model used the newly-activated read_file tool!"
  fi
else
  echo "Model content: $(echo "$TURN2_BODY" | jq -r '.choices[0].message.content // "no content"' | head -5)"
  echo "PASS: Provider accepted expanded tools[] (model chose to respond with text)"
fi

echo ""
echo "=== PROBE RESULT ==="
echo "Turn 1 (tools=[retrieve_tools]):           HTTP $TURN1_HTTP_CODE ✓"
echo "Turn 2 (tools=[retrieve_tools,read_file]):  HTTP $TURN2_HTTP_CODE ✓"
echo ""
echo "CONTRACT VERIFIED: Provider accepts tools[] growth across turns in the same conversation."
echo "The tool_call/tool_result in history does not need to match the previous request's tools[]."
