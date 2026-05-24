#!/usr/bin/env bash
set -euo pipefail

paths=(
  .env.example
  .github/workflows
  config
  crates
  docker
  docker-compose.yml
  README.md
  README-ru.md
)

forbidden_patterns=(
  '(^|[^A-Z0-9_])R2_ACCESS_KEY_ID([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])R2_SECRET_ACCESS_KEY([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])R2_ENDPOINT_URL([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])R2_BUCKET_NAME([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])R2_REGION([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])GEMINI_API_KEY([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])GOOGLE_GEMINI_API_KEY([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])OPENROUTER_SITE_URL([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])OPENROUTER_SITE_NAME([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])CHAT_MODEL_MAX_TOKENS([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])AGENT_MODEL_MAX_TOKENS([^A-Z0-9_]|$)'
  '(^|[^A-Z0-9_])SUB_AGENT_MODEL_MAX_TOKENS([^A-Z0-9_]|$)'
)

forbidden_rust_provider_settings=(
  'chatgpt_auth_path'
  'groq_api_key'
  'mistral_api_key'
  'minimax_api_key'
  'zai_api_key'
  'zai_api_base'
  'openrouter_api_key'
  'openrouter_site_url'
  'openrouter_site_name'
  'opencode_go_api_key'
  'opencode_go_api_base'
  'nvidia_api_key'
  'nvidia_api_base'
)

failed=0
for pattern in "${forbidden_patterns[@]}"; do
  if rg -n --pcre2 "${pattern}" "${paths[@]}" \
    --glob '!target/**' \
    --glob '!crates/oxide-agent-core/tests/tool_runtime_static_guards.rs'; then
    echo "forbidden legacy runtime env surface matched pattern: ${pattern}" >&2
    failed=1
  fi
done

for identifier in "${forbidden_rust_provider_settings[@]}"; do
  if rg -n "\\b${identifier}\\b" crates --glob '*.rs' --glob '!target/**'; then
    echo "forbidden global provider setting field matched identifier: ${identifier}" >&2
    failed=1
  fi
done

if [[ "${failed}" -ne 0 ]]; then
  exit 1
fi

echo "runtime env surface check passed"
