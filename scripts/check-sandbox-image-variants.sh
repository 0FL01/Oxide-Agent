#!/usr/bin/env bash
set -euo pipefail

sandbox_dir="sandbox"
variants=(minimal exec media dev)

for variant in "${variants[@]}"; do
  dockerfile="${sandbox_dir}/Dockerfile.${variant}"
  if [[ ! -f "${dockerfile}" ]]; then
    echo "missing sandbox image variant: ${dockerfile}" >&2
    exit 1
  fi
done

if [[ -f "${sandbox_dir}/Dockerfile.sandbox" ]]; then
  echo "legacy fat sandbox/Dockerfile.sandbox must not be used; select an explicit variant" >&2
  exit 1
fi

legacy_surface_paths=(
  .github/workflows
  docker
  docker-compose.telegram.yml
  docker-compose.web.yml
  README.md
  README-ru.md
)

existing_legacy_surface_paths=()
for path in "${legacy_surface_paths[@]}"; do
  if [[ -e "${path}" ]]; then
    existing_legacy_surface_paths+=("${path}")
  fi
done

if grep -RInE -- 'Dockerfile[.]sandbox' "${existing_legacy_surface_paths[@]}"; then
  echo "legacy fat sandbox Dockerfile must not be referenced by active deploy or runtime surfaces" >&2
  exit 1
fi

if ! grep -Eq -- 'dockerfile: sandbox/Dockerfile[.]dev' .github/workflows/ci-cd.yml; then
  echo "production deploy workflow must build the selected dev sandbox image variant" >&2
  exit 1
fi

normalized_dockerfile() {
  sed 's/#.*//' "$1" | tr '[:upper:]' '[:lower:]'
}

contains_token() {
  local text="$1"
  local token="$2"
  grep -Eq "(^|[^a-z0-9_.-])${token}([^a-z0-9_.-]|$)" <<<"${text}"
}

minimal_text="$(normalized_dockerfile "${sandbox_dir}/Dockerfile.minimal")"
for forbidden in ffmpeg python3 python3-pip pip yt-dlp nmap mtr chromium google-chrome firefox playwright browser-use; do
  if contains_token "${minimal_text}" "${forbidden}"; then
    echo "minimal sandbox image must not include '${forbidden}'" >&2
    exit 1
  fi
done

for variant in "${variants[@]}"; do
  text="$(normalized_dockerfile "${sandbox_dir}/Dockerfile.${variant}")"
  for forbidden_browser in chromium google-chrome firefox playwright browser-use; do
    if contains_token "${text}" "${forbidden_browser}"; then
      echo "sandbox image variant '${variant}' must not include browser package '${forbidden_browser}'" >&2
      exit 1
    fi
  done
done

media_text="$(normalized_dockerfile "${sandbox_dir}/Dockerfile.media")"
for required in ffmpeg python3 python3-pip yt-dlp; do
  if ! contains_token "${media_text}" "${required}"; then
    echo "media sandbox image must include '${required}'" >&2
    exit 1
  fi
done

dev_text="$(normalized_dockerfile "${sandbox_dir}/Dockerfile.dev")"
for required in ffmpeg python3 python3-pip yt-dlp nmap mtr; do
  if ! contains_token "${dev_text}" "${required}"; then
    echo "dev sandbox image must include '${required}'" >&2
    exit 1
  fi
done

if command -v docker >/dev/null 2>&1; then
  for variant in "${variants[@]}"; do
    docker build --check -f "${sandbox_dir}/Dockerfile.${variant}" .
  done
else
  echo "docker not found; skipped sandbox Dockerfile syntax checks" >&2
fi

echo "sandbox image variant checks passed"
