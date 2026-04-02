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
        candidate = _read_result_field(value, attr)
        if isinstance(candidate, str) and candidate.strip():
            return candidate.strip()
    return json.dumps(json_safe(value), ensure_ascii=True)


def extract_artifacts(value: Any) -> list[dict[str, Any]]:
    """Extract artifacts from result value."""
    for attr in ("artifacts", "files", "screenshots"):
        candidate = _read_result_field(value, attr)
        if candidate is None:
            continue
        safe = json_safe(candidate)
        if isinstance(safe, list):
            return [
                item if isinstance(item, dict) else {"value": item} for item in safe
            ]
    return []


def classify_run_result(value: Any) -> tuple[bool, str | None]:
    """Classify browser-use run result into success or internal failure."""
    is_done = _read_result_field(value, "is_done")
    is_successful = _read_result_field(value, "is_successful")
    errors = _read_result_field(value, "errors")
    final_result = _read_result_field(value, "final_result")
    last_error = _last_non_empty_error(errors)

    if is_done is False:
        return (
            False,
            last_error or "browser_use agent stopped before completing the task",
        )

    if is_successful is False:
        return (
            False,
            last_error
            or _clean_string(final_result)
            or "browser_use agent reported unsuccessful completion",
        )

    return True, None


def _read_result_field(value: Any, attr: str) -> Any:
    candidate = getattr(value, attr, None)
    if callable(candidate):
        try:
            candidate = candidate()
        except TypeError:
            return None
    return candidate


def _last_non_empty_error(errors: Any) -> str | None:
    if not isinstance(errors, list):
        return None

    for error in reversed(errors):
        cleaned = _clean_string(error)
        if cleaned:
            return cleaned
    return None


def _clean_string(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    cleaned = value.strip()
    return cleaned or None
