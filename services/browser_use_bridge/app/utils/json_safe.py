"""JSON-safe serialization utilities."""

from __future__ import annotations

import json
from typing import Any


def json_safe(value: Any) -> Any:
    """Convert value to JSON-safe format recursively."""
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, dict):
        return {str(key): json_safe(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [json_safe(item) for item in value]
    if hasattr(value, "model_dump"):
        return json_safe(value.model_dump())
    if hasattr(value, "dict"):
        return json_safe(value.dict())
    if hasattr(value, "__dict__"):
        return json_safe(vars(value))
    return str(value)


def stringify_result(value: Any) -> str:
    """Convert result to human-readable string."""
    if value is None:
        return "Task completed."
    if isinstance(value, str):
        text = value.strip()
        return text or "Task completed."
    for attr in ("final_result", "result", "summary", "message"):
        candidate = getattr(value, attr, None)
        if isinstance(candidate, str) and candidate.strip():
            return candidate.strip()
    return json.dumps(json_safe(value), ensure_ascii=True)


def extract_artifacts(value: Any) -> list[dict[str, Any]]:
    """Extract artifacts from result value."""
    for attr in ("artifacts", "files", "screenshots"):
        candidate = getattr(value, attr, None)
        if candidate is None:
            continue
        safe = json_safe(candidate)
        if isinstance(safe, list):
            return [
                item if isinstance(item, dict) else {"value": item} for item in safe
            ]
    return []
