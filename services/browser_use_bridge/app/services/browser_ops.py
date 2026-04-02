"""Browser operations: creation, screenshots, content extraction."""

from __future__ import annotations

import base64
import inspect
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from fastapi import HTTPException

from app.models.internal import ProfileRecord
from app.utils.browser_utils import (
    browser_state_snapshot,
    invoke_with_supported_kwargs,
    maybe_await,
    resolve_browser_page,
)

try:
    import browser_use as browser_use_module
except ImportError:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None

Browser = getattr(browser_use_module, "Browser", None)

PROFILE_PATH_CANDIDATES = (
    "user_data_dir",
    "profile_path",
    "profile_dir",
    "browser_profile_path",
    "browser_user_data_dir",
)


def create_browser(profile: ProfileRecord | None, *, keep_alive: bool = False) -> Any:
    """Create browser instance with optional profile persistence."""
    if Browser is None:
        raise RuntimeError("Browser is unavailable in installed browser_use package")

    profile_path = profile.browser_data_dir if profile is not None else None
    try:
        signature = inspect.signature(Browser)
    except (TypeError, ValueError):
        signature = None

    if signature is not None:
        supports_var_kwargs = any(
            param.kind == inspect.Parameter.VAR_KEYWORD
            for param in signature.parameters.values()
        )
        kwargs: dict[str, Any] = {}

        if keep_alive and (supports_var_kwargs or "keep_alive" in signature.parameters):
            kwargs["keep_alive"] = True

        if profile_path is None:
            return Browser(**kwargs)

        for candidate in PROFILE_PATH_CANDIDATES:
            if supports_var_kwargs or candidate in signature.parameters:
                kwargs[candidate] = profile_path
                return Browser(**kwargs)

        raise RuntimeError(
            "installed browser_use Browser constructor does not expose a supported persistent profile path argument"
        )

    if profile_path is None:
        if keep_alive:
            try:
                return Browser(keep_alive=True)
            except TypeError:
                pass
        return Browser()

    for candidate in PROFILE_PATH_CANDIDATES:
        kwargs = {candidate: profile_path}
        if keep_alive:
            kwargs["keep_alive"] = True
        try:
            return Browser(**kwargs)
        except TypeError:
            if keep_alive:
                try:
                    return Browser(**{candidate: profile_path})
                except TypeError:
                    pass
            continue

    raise RuntimeError(
        "installed browser_use Browser constructor does not expose a supported persistent profile path argument"
    )


async def close_browser(browser: Any, *, kill: bool = False) -> None:
    """Safely close browser instance.

    When `kill=True`, prefer hard shutdown so kept-alive runtimes do not leak.
    """
    if browser is None:
        return

    method_names = (
        ("kill", "close", "quit", "stop") if kill else ("stop", "close", "quit", "kill")
    )
    for method_name in method_names:
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
        evaluate = getattr(page, "evaluate", None)
        if callable(evaluate):
            result = await maybe_await(
                evaluate("() => document.documentElement.outerHTML")
            )
            if isinstance(result, str) and result.strip():
                return result

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

    state = await browser_state_snapshot(browser, include_screenshot=False)
    for key in ("html", "page_html", "content"):
        value = state.get(key)
        if isinstance(value, str) and value.strip():
            return value

    raise HTTPException(
        status_code=500,
        detail="browser_use bridge could not extract HTML from active session",
    )


async def _extract_text(browser: Any, page: Any) -> str:
    """Extract text content from page."""
    if page is not None:
        evaluate = getattr(page, "evaluate", None)
        if callable(evaluate):
            result = await maybe_await(
                evaluate(
                    "() => document.body ? document.body.innerText : document.documentElement.innerText"
                )
            )
            if isinstance(result, str) and result.strip():
                return result

        inner_text = getattr(page, "inner_text", None)
        if callable(inner_text):
            result = await maybe_await(inner_text("body"))
            if isinstance(result, str) and result.strip():
                return result

    browser_text = getattr(browser, "get_text", None)
    if callable(browser_text):
        result = await maybe_await(browser_text())
        if isinstance(result, str) and result.strip():
            return result

    browser_state_text = getattr(browser, "get_state_as_text", None)
    if callable(browser_state_text):
        result = await maybe_await(browser_state_text())
        if isinstance(result, str) and result.strip():
            return result

    state = await browser_state_snapshot(browser, include_screenshot=False)
    for key in ("text", "page_text", "content"):
        value = state.get(key)
        if isinstance(value, str) and value.strip():
            return value

    raise HTTPException(
        status_code=500,
        detail="browser_use bridge could not extract page content from active session",
    )


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

    browser_take_screenshot = getattr(browser, "take_screenshot", None)
    if callable(browser_take_screenshot):
        screenshot_methods.append(browser_take_screenshot)
    browser_screenshot = getattr(browser, "screenshot", None)
    if callable(browser_screenshot):
        screenshot_methods.append(browser_screenshot)
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
        if isinstance(result, str) and result.strip():
            try:
                path.write_bytes(base64.b64decode(result))
                break
            except Exception:
                pass

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
