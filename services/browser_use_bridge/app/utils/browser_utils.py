"""Browser-related utilities."""

from __future__ import annotations

import inspect
from typing import Any

from app.utils.json_safe import json_safe


BROWSER_RUNTIME_UNAVAILABLE_PATTERNS = [
    "cdp client not initialized",
    "browser is not initialized",
    "browser not initialized",
    "root cdp client not initialized",
    "page is not initialized",
    "page not initialized",
    "context is not initialized",
    "context not initialized",
    "sessionmanager not initialized",
    "session manager not initialized",
    "no valid agent focus available",
    "no current target found",
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
    for attr in ("get_current_page", "page", "current_page"):
        candidate = getattr(browser, attr, None)
        if candidate is None:
            continue
        if callable(candidate):
            try:
                candidate = await maybe_await(candidate())
            except Exception as error:
                if is_browser_session_unavailable_error(error):
                    raise RuntimeError(
                        f"browser session is not alive: {error}"
                    ) from error
                return None
        if candidate is not None:
            return candidate
    return None


async def start_browser_runtime(browser: Any) -> None:
    """Start browser runtime if the installed browser_use surface exposes it."""
    start = getattr(browser, "start", None)
    if callable(start):
        await maybe_await(start())


async def ensure_browser_session_alive(browser: Any) -> None:
    """Verify that a browser session still has a usable runtime/page."""
    alive, reason = await probe_browser_session_state(browser)
    if alive:
        return
    raise RuntimeError(reason or "browser session is not alive")


async def ensure_browser_runtime_ready(browser: Any) -> None:
    """Verify that a browser runtime is connected enough to start an agent run."""
    ready, reason = await probe_browser_runtime_ready(browser)
    if ready:
        return
    raise RuntimeError(reason or "browser runtime is not ready")


async def probe_browser_runtime_ready(browser: Any) -> tuple[bool, str | None]:
    """Probe startup readiness before the first agent step runs."""
    if browser is None:
        return False, "browser runtime is not ready: browser handle is missing"

    if await _object_is_closed(browser):
        return False, "browser runtime is not ready: browser runtime is closed"

    try:
        await start_browser_runtime(browser)
    except Exception as error:
        if is_transient_browser_ready_error(error):
            return False, f"browser runtime is not ready: {error}"
        raise

    if await _has_live_runtime_handle(browser):
        return True, None

    try:
        page = await resolve_browser_page(browser)
    except RuntimeError as error:
        return False, str(error)
    if await _object_is_closed(page):
        return False, "browser runtime is not ready: browser page is closed"
    if page is not None:
        return True, None

    current_url = getattr(browser, "get_current_page_url", None)
    if callable(current_url):
        try:
            await maybe_await(current_url())
            return True, None
        except Exception as error:
            if is_transient_browser_ready_error(error):
                return False, f"browser runtime is not ready: {error}"
            raise

    try:
        state = await browser_state_snapshot(browser, include_screenshot=False)
    except RuntimeError as error:
        return False, str(error)
    if state:
        return True, None

    return False, "browser runtime is not ready: browser runtime handle is unavailable"


async def probe_browser_session_state(browser: Any) -> tuple[bool, str | None]:
    """Probe browser runtime liveness without raising on ordinary dead-session cases."""
    if browser is None:
        return False, "browser session is not alive: browser handle is missing"

    if await _object_is_closed(browser):
        return False, "browser session is not alive: browser runtime is closed"

    try:
        page = await resolve_browser_page(browser)
    except RuntimeError as error:
        return False, str(error)
    if await _object_is_closed(page):
        return False, "browser session is not alive: browser page is closed"
    if page is not None:
        return True, None

    if await _has_live_runtime_handle(browser):
        return True, None

    try:
        current_url = getattr(browser, "get_current_page_url", None)
        if callable(current_url):
            url = await maybe_await(current_url())
            if isinstance(url, str) and url.strip():
                return True, None
        state = await browser_state_snapshot(browser, include_screenshot=False)
    except RuntimeError as error:
        return False, str(error)

    if not state:
        return False, "browser session is not alive: browser runtime is unavailable"

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
        current_url = getattr(browser, "get_current_page_url", None)
        if callable(current_url):
            try:
                value = await maybe_await(current_url())
            except Exception:
                value = None
            if isinstance(value, str) and value.strip():
                return value.strip()

        try:
            state = await browser_state_snapshot(browser, include_screenshot=False)
        except RuntimeError:
            return None

        for key in ("url", "current_url", "page_url"):
            value = state.get(key)
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


async def _has_live_runtime_handle(browser: Any) -> bool:
    return any(
        getattr(browser, attr, None) is not None
        for attr in ("session_manager", "_cdp_client_root")
    )


async def browser_state_snapshot(
    browser: Any, *, include_screenshot: bool = False
) -> dict[str, Any]:
    """Read browser state summary if the runtime exposes it."""
    get_browser_state_summary = getattr(browser, "get_browser_state_summary", None)
    if callable(get_browser_state_summary):
        try:
            state = await maybe_await(
                invoke_with_supported_kwargs(
                    get_browser_state_summary,
                    include_screenshot=include_screenshot,
                )
            )
        except Exception as error:
            if is_browser_session_unavailable_error(error):
                raise RuntimeError(f"browser session is not alive: {error}") from error
            return {}

        safe_state = json_safe(state)
        return safe_state if isinstance(safe_state, dict) else {}

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
