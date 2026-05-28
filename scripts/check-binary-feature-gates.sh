#!/usr/bin/env bash
set -euo pipefail

metadata="$(cargo metadata --no-deps --format-version 1)"

METADATA_JSON="${metadata}" python3 - <<'PY'
import json
import os
import sys

metadata = json.loads(os.environ["METADATA_JSON"])

expected = {
    ("oxide-agent-telegram-bot", "oxide-agent-telegram-bot"): [
        "storage-s3-r2",
        "transport-telegram",
    ],
    ("oxide-agent-transport-web", "oxide-agent-web-console"): [],
    ("oxide-agent-web-ui", "oxide-agent-web-ui"): [],
    ("oxide-agent-telegram-bot", "chatgpt-login"): [
        "llm-chatgpt",
    ],
    ("oxide-agent-sandboxd", "oxide-agent-sandboxd"): [
        "sandbox-daemon",
    ],
}

seen = set()
failures = []
unexpected = []
for package in metadata["packages"]:
    package_name = package["name"]
    for target in package["targets"]:
        if "bin" not in target["kind"]:
            continue
        key = (package_name, target["name"])
        if key not in expected:
            unexpected.append(key)
            continue
        seen.add(key)
        actual = sorted(target.get("required-features", []))
        wanted = expected[key]
        if actual != wanted:
            failures.append(f"{package_name}/{target['name']}: expected {wanted}, got {actual}")

missing = sorted(set(expected) - seen)
if missing:
    failures.append(f"missing expected binary targets: {missing}")
if unexpected:
    failures.append(f"unexpected binary targets must be added to the feature-gate contract: {sorted(unexpected)}")

if failures:
    print("binary feature gate check failed:", file=sys.stderr)
    for failure in failures:
        print(f"  - {failure}", file=sys.stderr)
    sys.exit(1)

print("binary feature gate check passed")
PY
