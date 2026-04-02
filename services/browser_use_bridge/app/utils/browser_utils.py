"""Browser-related utilities."""

from __future__ import annotations

import asyncio
import inspect
from typing import Any

from app.utils.json_safe import json_safe


async def maybe_await(value: Any) -> Any:
    """Await value if it's awaitable, otherwise return as-is."""
    if inspect.isawaitable(value):
        return await value
    return value


def invoke_with_supported_kwargs(method: Any, **kwargs: Any) -> Any:
    """Invoke method with only kwargs it supports (filter by signature)."""
    try:
        signature = inspect.signature(method)
    except (TypeError, ValueError):
        return method(**kwargs)

    if any(
        param.kind == inspect.Parameter.VAR_KEYWORD
        for param in signature.parameters.values()
    ):
        return method(**kwargs)

    filtered = {
        name: value for name, value in kwargs.items() if name in signature.parameters
    }
    return method(**filtered)


async def resolve_browser_page(browser: Any) -> Any | None:
    """Resolve page object from browser instance."""
    for attr in ("page", "current_page"):
        candidate = getattr(browser, attr, None)
        if candidate is None:
            continue
        if callable(candidate):
            candidate = await maybe_await(candidate())
        if candidate is not None:
            return candidate
    return None


async def infer_url(browser: Any, result: Any) -> str | None:
    """Infer URL from browser or result object."""
    for source in (result, browser):
        if source is None:
            continue
        for attr in ("final_url", "current_url", "url"):
            candidate = getattr(source, attr, None)
            if isinstance(candidate, str) and candidate.strip():
                return candidate.strip()
            if callable(candidate):
                value = await maybe_await(candidate())
                if isinstance(value, str) and value.strip():
                    return value.strip()

    if browser is not None:
        state_method = getattr(browser, "get_state", None)
        if callable(state_method):
            try:
                state = await maybe_await(state_method())
            except Exception:
                return None
            safe_state = json_safe(state)
            if isinstance(safe_state, dict):
                for key in ("url", "current_url", "page_url"):
                    value = safe_state.get(key)
                    if isinstance(value, str) and value.strip():
                        return value.strip()
    return None


def is_transient_browser_ready_error(error: Exception) -> bool:
    """Check if error is a transient browser initialization error."""
    message = str(error).strip().lower()
    if not message:
        return False

    transient_patterns = [
        "cdp client not initialized",
        "browser is not initialized",
        "browser not initialized",
        "page is not initialized",
        "page not initialized",
        "context is not initialized",
        "context not initialized",
        "target page, context or browser has been closed",
        "target closed",
        "browser has been closed",
    ]
    return any(pattern in message for pattern in transient_patterns)
