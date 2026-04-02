"""Browser operations: creation, screenshots, content extraction."""

from __future__ import annotations

import inspect
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from fastapi import HTTPException

from app.models.internal import ProfileRecord
from app.utils.browser_utils import (
    maybe_await,
    resolve_browser_page,
    invoke_with_supported_kwargs,
)
from app.utils.json_safe import json_safe

try:
    import browser_use as browser_use_module
except ImportError:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None

Browser = getattr(browser_use_module, "Browser", None)


def create_browser(profile: ProfileRecord | None) -> Any:
    """Create browser instance with optional profile persistence."""
    if Browser is None:
        raise RuntimeError("Browser is unavailable in installed browser_use package")

    if profile is None:
        return Browser()

    profile_path = profile.browser_data_dir
    try:
        signature = inspect.signature(Browser)
    except (TypeError, ValueError):
        signature = None

    if signature is not None:
        if any(
            param.kind == inspect.Parameter.VAR_KEYWORD
            for param in signature.parameters.values()
        ):
            return Browser(user_data_dir=profile_path)

        for candidate in (
            "user_data_dir",
            "profile_path",
            "profile_dir",
            "browser_profile_path",
            "browser_user_data_dir",
        ):
            if candidate in signature.parameters:
                return Browser(**{candidate: profile_path})

        raise RuntimeError(
            "installed browser_use Browser constructor does not expose a supported persistent profile path argument"
        )

    # Fallback: try common parameter names
    for candidate in (
        "user_data_dir",
        "profile_path",
        "profile_dir",
        "browser_profile_path",
        "browser_user_data_dir",
    ):
        try:
            return Browser(**{candidate: profile_path})
        except TypeError:
            continue

    raise RuntimeError(
        "installed browser_use Browser constructor does not expose a supported persistent profile path argument"
    )


async def close_browser(browser: Any) -> None:
    """Safely close browser instance."""
    if browser is None:
        return
    for method_name in ("close", "stop", "quit"):
        method = getattr(browser, method_name, None)
        if callable(method):
            try:
                await maybe_await(method())
            except Exception:
                pass
            return


async def extract_content(
    browser: Any, content_format: str, max_chars: int | None = None
) -> tuple[str, bool, int]:
    """Extract content from browser page.

    Returns: (content, was_truncated, total_chars)
    """
    page = await resolve_browser_page(browser)

    if content_format == "html":
        content = await _extract_html(browser, page)
    else:
        content = await _extract_text(browser, page)

    total_chars = len(content)
    if max_chars is not None and total_chars > max_chars:
        return content[:max_chars], True, total_chars
    return content, False, total_chars


async def _extract_html(browser: Any, page: Any) -> str:
    """Extract HTML content from page."""
    if page is not None:
        content_method = getattr(page, "content", None)
        if callable(content_method):
            result = await maybe_await(content_method())
            if isinstance(result, str) and result.strip():
                return result

    browser_html = getattr(browser, "get_html", None)
    if callable(browser_html):
        result = await maybe_await(browser_html())
        if isinstance(result, str) and result.strip():
            return result

    state = await _browser_state(browser)
    for key in ("html", "page_html", "content"):
        value = state.get(key)
        if isinstance(value, str) and value.strip():
            return value

    if page is not None:
        evaluate = getattr(page, "evaluate", None)
        if callable(evaluate):
            result = await maybe_await(evaluate("document.documentElement.outerHTML"))
            if isinstance(result, str) and result.strip():
                return result

    raise HTTPException(
        status_code=500,
        detail="browser_use bridge could not extract HTML from active session",
    )


async def _extract_text(browser: Any, page: Any) -> str:
    """Extract text content from page."""
    if page is not None:
        inner_text = getattr(page, "inner_text", None)
        if callable(inner_text):
            result = await maybe_await(inner_text("body"))
            if isinstance(result, str) and result.strip():
                return result

        evaluate = getattr(page, "evaluate", None)
        if callable(evaluate):
            result = await maybe_await(
                evaluate(
                    "document.body ? document.body.innerText : document.documentElement.innerText"
                )
            )
            if isinstance(result, str) and result.strip():
                return result

    browser_text = getattr(browser, "get_text", None)
    if callable(browser_text):
        result = await maybe_await(browser_text())
        if isinstance(result, str) and result.strip():
            return result

    state = await _browser_state(browser)
    for key in ("text", "page_text", "content"):
        value = state.get(key)
        if isinstance(value, str) and value.strip():
            return value

    raise HTTPException(
        status_code=500,
        detail="browser_use bridge could not extract page content from active session",
    )


async def _browser_state(browser: Any) -> dict[str, Any]:
    """Get browser state as dict."""
    get_state = getattr(browser, "get_state", None)
    if callable(get_state):
        try:
            state = await maybe_await(get_state())
        except Exception:
            return {}
        safe_state = json_safe(state)
        if isinstance(safe_state, dict):
            return safe_state
    return {}


async def take_screenshot(
    browser: Any, artifacts_dir: Path, session_id: str, full_page: bool
) -> dict[str, Any]:
    """Take screenshot and save to artifacts directory."""
    session_artifacts_dir = artifacts_dir / session_id
    session_artifacts_dir.mkdir(parents=True, exist_ok=True)
    file_name = (
        f"screenshot-{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')}.png"
    )
    path = session_artifacts_dir / file_name
    page = await resolve_browser_page(browser)

    screenshot_methods = []
    browser_screenshot = getattr(browser, "screenshot", None)
    if callable(browser_screenshot):
        screenshot_methods.append(browser_screenshot)
    browser_take_screenshot = getattr(browser, "take_screenshot", None)
    if callable(browser_take_screenshot):
        screenshot_methods.append(browser_take_screenshot)
    if page is not None:
        page_screenshot = getattr(page, "screenshot", None)
        if callable(page_screenshot):
            screenshot_methods.append(page_screenshot)

    for method in screenshot_methods:
        try:
            result = await maybe_await(
                invoke_with_supported_kwargs(
                    method, path=str(path), full_page=full_page
                )
            )
        except Exception:
            continue

        if path.exists():
            break
        if isinstance(result, (bytes, bytearray)):
            path.write_bytes(bytes(result))
            break

    if not path.exists():
        raise HTTPException(
            status_code=500,
            detail="browser_use bridge could not create screenshot from active session",
        )

    from app.utils.time import utc_now

    return {
        "kind": "screenshot",
        "path": str(path),
        "full_page": full_page,
        "size_bytes": path.stat().st_size,
        "created_at": utc_now(),
    }
