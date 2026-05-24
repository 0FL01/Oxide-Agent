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
)

failed=0
for pattern in "${forbidden_patterns[@]}"; do
  if rg -n --pcre2 "${pattern}" "${paths[@]}" --glob '!target/**'; then
    echo "forbidden legacy runtime env surface matched pattern: ${pattern}" >&2
    failed=1
  fi
done

if [[ "${failed}" -ne 0 ]]; then
  exit 1
fi

echo "runtime env surface check passed"
