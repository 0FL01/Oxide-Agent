"""Browser-related utilities."""

from __future__ import annotations

import asyncio
import inspect
from typing import Any

from app.utils.json_safe import json_safe


BROWSER_RUNTIME_UNAVAILABLE_PATTERNS = [
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
    "browser session is not alive",
]


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


async def ensure_browser_session_alive(browser: Any) -> None:
    """Verify that a browser session still has a usable runtime/page."""
    alive, reason = await probe_browser_session_state(browser)
    if alive:
        return
    raise RuntimeError(reason or "browser session is not alive")


async def probe_browser_session_state(browser: Any) -> tuple[bool, str | None]:
    """Probe browser runtime liveness without raising on ordinary dead-session cases."""
    if browser is None:
        return False, "browser session is not alive: browser handle is missing"

    if await _object_is_closed(browser):
        return False, "browser session is not alive: browser runtime is closed"

    page = await resolve_browser_page(browser)
    if await _object_is_closed(page):
        return False, "browser session is not alive: browser page is closed"

    try:
        state = await _browser_state(browser)
    except RuntimeError as error:
        return False, str(error)

    if page is None and not state:
        return False, "browser session is not alive: browser page is unavailable"

    return True, None


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

    return any(pattern in message for pattern in BROWSER_RUNTIME_UNAVAILABLE_PATTERNS)


def is_browser_session_unavailable_error(error: Exception | str) -> bool:
    """Check if an error indicates that a browser session/runtime is no longer alive."""
    message = error if isinstance(error, str) else str(error)
    message = message.strip().lower()
    if not message:
        return False

    return any(pattern in message for pattern in BROWSER_RUNTIME_UNAVAILABLE_PATTERNS)


async def _object_is_closed(candidate: Any) -> bool:
    if candidate is None:
        return False

    for attr in ("is_closed", "closed"):
        value = getattr(candidate, attr, None)
        if value is None:
            continue
        if callable(value):
            try:
                value = await maybe_await(value())
            except Exception as error:
                return is_browser_session_unavailable_error(error)
        if isinstance(value, bool):
            return value

    return False


async def _browser_state(browser: Any) -> dict[str, Any]:
    get_state = getattr(browser, "get_state", None)
    if not callable(get_state):
        return {}

    try:
        state = await maybe_await(get_state())
    except Exception as error:
        if is_browser_session_unavailable_error(error):
            raise RuntimeError(f"browser session is not alive: {error}") from error
        return {}

    safe_state = json_safe(state)
    return safe_state if isinstance(safe_state, dict) else {}
