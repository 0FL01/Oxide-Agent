#!/usr/bin/env python3
"""Browser Live sidecar: thin HTTP adapter over the chrome-agent CLI.

The sidecar exposes the stable REST contract that Oxide expects, authorizes
requests, and translates each request into a chrome-agent invocation.
chrome-agent (already installed in the image) controls headless Chromium
directly, so this wrapper deliberately avoids implementing CDP by hand.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse


ONE_PIXEL_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
)

IDEMPOTENCY_KEY_HEADER = "Idempotency-Key"


class SidecarState:
    def __init__(self) -> None:
        self.addr = os.getenv("BROWSER_AGENT_SIDECAR_ADDR", "0.0.0.0")
        self.port = int(os.getenv("BROWSER_AGENT_SIDECAR_PORT", "8787"))
        self.token = os.getenv("BROWSER_AGENT_SIDECAR_TOKEN", "").strip()
        self.artifact_dir = Path(
            os.getenv("BROWSER_AGENT_ARTIFACT_DIR", "/var/lib/oxide-browser/artifacts")
        )
        self.profile_dir = Path(
            os.getenv("BROWSER_AGENT_PROFILE_DIR", "/tmp/oxide-browser-profiles")
        )
        self.chrome_bin = shutil.which("chromium") or os.getenv("CHROME_BIN", "/usr/bin/chromium")
        self.sessions: dict[str, dict[str, Any]] = {}
        self.artifact_dir.mkdir(parents=True, exist_ok=True)
        self.profile_dir.mkdir(parents=True, exist_ok=True)

    def reset(self) -> None:
        for child in self.profile_dir.iterdir():
            shutil.rmtree(child, ignore_errors=True)
        self.sessions.clear()


STATE = SidecarState()


def request_id() -> str:
    return f"req-{uuid.uuid4().hex[:12]}"


def safe(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in "-_." else "-" for ch in value.strip()) or "unknown"


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def sha256_of_file(path: Path) -> str:
    h = hashlib.sha256()
    try:
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(8192), b""):
                h.update(chunk)
        return h.hexdigest()
    except OSError:
        return "sha256-unavailable"


def session_id_from_path(path: str, suffix: str) -> str | None:
    prefix = "/sessions/"
    if not path.startswith(prefix) or (suffix and not path.endswith(suffix)):
        return None
    value = path[len(prefix):]
    if suffix:
        value = value[:-len(suffix)]
    return value.strip("/") or None


def run_chrome_agent(browser: str, args: list[str], timeout: int = 60) -> dict[str, Any]:
    """Run chrome-agent --json and return parsed JSON, normalizing errors."""
    cmd = ["chrome-agent", "--browser", browser, "--json"] + args
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return {
            "ok": False,
            "error": {
                "code": "timeout",
                "message": f"chrome-agent command timed out after {timeout}s",
                "retryable": True,
                "hint": "retry after observing browser state",
            },
        }
    except FileNotFoundError:
        return {
            "ok": False,
            "error": {
                "code": "sidecar_not_ready",
                "message": "chrome-agent binary not found",
                "retryable": True,
                "hint": "ensure chrome-agent is installed in PATH",
            },
        }

    stdout = result.stdout.strip()
    if not stdout:
        return {
            "ok": False,
            "error": {
                "code": "invalid_response",
                "message": "chrome-agent returned empty output",
                "retryable": False,
                "hint": result.stderr.strip()[:200],
            },
        }

    try:
        data = json.loads(stdout)
    except json.JSONDecodeError:
        return {
            "ok": False,
            "error": {
                "code": "invalid_response",
                "message": "chrome-agent returned non-JSON output",
                "retryable": False,
                "hint": stdout[:200],
            },
        }

    # Normalize chrome-agent string errors into the Rust contract envelope.
    if data.get("ok") is False and isinstance(data.get("error"), str):
        data["error"] = {
            "code": "chrome_agent_error",
            "message": data["error"],
            "retryable": False,
            "hint": data.get("hint", "") or "",
        }
    return data


def chrome_agent_available() -> bool:
    return shutil.which("chrome-agent") is not None


def session_artifact_dir(task_id: str, session_id: str) -> Path:
    path = STATE.artifact_dir / safe(task_id) / safe(session_id)
    path.mkdir(parents=True, exist_ok=True)
    return path


def capture_screenshot(session_id: str, fresh: bool = True) -> dict[str, Any]:
    """Capture a screenshot via chrome-agent and copy it into the artifact dir.

    Each successful capture increments ``screenshot_seq`` and produces a unique
    ``screenshot_id``.  When ``fresh`` is false, the last cached screenshot is
    returned without invoking chrome-agent, allowing cheap stale observations.
    """
    session = STATE.sessions.get(session_id, {})
    task_id = session.get("task_id", "browser-task")
    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})

    if not fresh:
        last = session.get("last_screenshot")
        if last is not None:
            return last

    result = run_chrome_agent(session_id, ["screenshot"], timeout=30)
    if not result.get("ok"):
        artifact_dir = session_artifact_dir(task_id, session_id)
        dest = artifact_dir / "latest.png"
        dest.write_bytes(ONE_PIXEL_PNG)
        return {
            "screenshot_id": f"shot-{session_id}-0",
            "artifact_uri": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/latest.png",
            "mime_type": "image/png",
            "width": viewport["width"],
            "height": viewport["height"],
            "sha256": sha256_of_file(dest),
            "captured_at": now_iso(),
            "redacted": True,
            "byte_size": len(ONE_PIXEL_PNG),
        }

    src_path = Path(result["path"])
    artifact_dir = session_artifact_dir(task_id, session_id)
    dest = artifact_dir / "latest.png"
    if src_path.exists():
        shutil.copy2(src_path, dest)
    else:
        dest.write_bytes(ONE_PIXEL_PNG)

    screenshot_seq = session.get("screenshot_seq", 0) + 1
    session["screenshot_seq"] = screenshot_seq
    screenshot = {
        "screenshot_id": f"shot-{session_id}-{screenshot_seq}",
        "artifact_uri": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/latest.png",
        "mime_type": "image/png",
        "width": viewport["width"],
        "height": viewport["height"],
        "sha256": sha256_of_file(dest),
        "captured_at": now_iso(),
        "redacted": False,
        "byte_size": dest.stat().st_size if dest.exists() else 0,
    }
    session["last_screenshot"] = screenshot
    return screenshot


def parse_snapshot(snapshot: str) -> list[dict[str, Any]]:
    """Convert chrome-agent snapshot text into a flat a11y summary."""
    items: list[dict[str, Any]] = []
    for raw in snapshot.splitlines():
        line = raw.rstrip()
        if not line.strip():
            continue
        stripped = line.lstrip()
        if not stripped.startswith("uid="):
            continue
        indent = len(line) - len(stripped)
        # e.g. uid=n14 link "Learn more"
        #      uid=n11 heading "Example Domain" level=1
        uid = ""
        role = ""
        text = ""
        if " " in stripped:
            uid_part, rest = stripped.split(" ", 1)
            uid = uid_part.split("=", 1)[1] if "=" in uid_part else ""
            # role is the next token
            parts = rest.split(" ", 1)
            role = parts[0]
            remainder = parts[1] if len(parts) > 1 else ""
            if '"' in remainder:
                text = remainder.split('"', 1)[1].rsplit('"', 1)[0]
        items.append({
            "uid": uid,
            "role": role,
            "text": text,
            "depth": indent // 2,
        })
    return items


def summarize_network(requests: list[dict[str, Any]], limit: int = 20) -> dict[str, Any]:
    """Map chrome-agent network requests to the Rust NetworkSummary shape."""
    failures: list[dict[str, Any]] = []
    for req in requests:
        status = req.get("status")
        if isinstance(status, int) and status >= 400:
            failures.append({
                "timestamp": req.get("timestamp", now_iso()),
                "method": req.get("method", "GET"),
                "url_redacted": req.get("url", ""),
                "status": status,
                "resource_type": req.get("resource_type", "other"),
                "error_text": req.get("error_text"),
            })
    return {
        "failed_count": len(failures),
        "recent_failures": failures[:limit],
    }


def summarize_console(messages: list[dict[str, Any]], limit: int = 20) -> dict[str, Any]:
    """Map chrome-agent console messages to the Rust ConsoleSummary shape."""
    errors: list[dict[str, Any]] = []
    warnings: list[dict[str, Any]] = []
    for msg in messages:
        level = msg.get("level", "info")
        entry = {
            "timestamp": msg.get("timestamp", now_iso()),
            "level": level,
            "text_redacted": msg.get("text", ""),
            "source": msg.get("source"),
            "line": msg.get("line"),
        }
        if level == "error":
            errors.append(entry)
        elif level == "warning":
            warnings.append(entry)
    return {
        "error_count": len(errors),
        "warning_count": len(warnings),
        "recent_errors": errors[:limit],
    }


def _network_item_key(item: dict[str, Any]) -> str:
    return "|".join([
        str(item.get("timestamp", "")),
        str(item.get("method", "")),
        str(item.get("url", "")),
        str(item.get("status", "")),
        str(item.get("resource_type", "")),
        str(item.get("error_text", "")),
    ])


def _console_item_key(item: dict[str, Any]) -> str:
    return "|".join([
        str(item.get("timestamp", "")),
        str(item.get("level", "")),
        str(item.get("text", "")),
        str(item.get("source", "")),
        str(item.get("line", "")),
    ])


def _merge_history(
    session: dict[str, Any],
    key: str,
    fresh_items: list[dict[str, Any]],
    action_seq: int,
    max_items: int = 1000,
) -> None:
    """Append new network/console items to the session history, deduplicating by content.

    Each entry stores the captured item and the action_seq at which it was observed,
    so debug endpoints can filter with ``since_action_seq``.
    """
    history: list[dict[str, Any]] = session.setdefault(key, [])
    seen: set[str] = set()
    for entry in history:
        item = entry["item"]
        seen.add(_network_item_key(item) if key == "network_history" else _console_item_key(item))
    for item in fresh_items:
        item_key = _network_item_key(item) if key == "network_history" else _console_item_key(item)
        if item_key in seen:
            continue
        history.append({"item": item, "action_seq": action_seq})
        seen.add(item_key)
    if len(history) > max_items:
        session[key] = history[-max_items:]


def build_network_debug_payload(
    history: list[dict[str, Any]],
    since_action_seq: int,
    filter_value: str,
    level: str,
    include_bodies: bool,
    limit: int,
) -> dict[str, Any]:
    """Build the NetworkDebugPayload shape from accumulated network history."""
    del level  # only summary is supported by the chrome-agent wrapper contract
    del include_bodies  # chrome-agent wrapper does not expose response bodies
    items = [entry["item"] for entry in history if entry["action_seq"] >= since_action_seq]
    if filter_value == "failed":
        items = [
            item for item in items
            if (isinstance(item.get("status"), int) and item["status"] >= 400)
            or item.get("error_text")
        ]
    elif filter_value == "xhr":
        items = [item for item in items if (item.get("resource_type") or "").lower() == "xhr"]
    elif filter_value == "fetch":
        items = [item for item in items if (item.get("resource_type") or "").lower() == "fetch"]
    elif filter_value == "document":
        items = [item for item in items if (item.get("resource_type") or "").lower() == "document"]
    failures = [item for item in items if isinstance(item.get("status"), int) and item["status"] >= 400]
    items = items[-limit:]
    return {
        "failed_count": len(failures),
        "items": items,
        "artifact_uri": None,
    }


def build_console_debug_payload(
    history: list[dict[str, Any]],
    since_action_seq: int,
    level: str,
    min_level: str,
    limit: int,
) -> dict[str, Any]:
    """Build the ConsoleDebugPayload shape from accumulated console history."""
    del level  # only summary is supported by the chrome-agent wrapper contract
    level_rank = {"verbose": 0, "info": 1, "warning": 2, "error": 3}
    min_rank = level_rank.get(min_level, 3)
    items = [entry["item"] for entry in history if entry["action_seq"] >= since_action_seq]
    items = [item for item in items if level_rank.get(item.get("level", "info"), 1) >= min_rank]
    error_count = sum(1 for item in items if item.get("level") == "error")
    warning_count = sum(1 for item in items if item.get("level") == "warning")
    items = items[-limit:]
    return {
        "error_count": error_count,
        "warning_count": warning_count,
        "items": items,
        "artifact_uri": None,
    }


def build_observation(
    session_id: str,
    chrome_output: dict[str, Any],
    action_seq: int = 0,
    include_network: bool = True,
    include_console: bool = True,
    fresh: bool = True,
    max_debug_items: int = 20,
) -> dict[str, Any]:
    """Build a full BrowserObservation from chrome-agent output.

    Each fresh observation increments ``observation_seq`` and produces a unique
    ``observation_id``.  Non-fresh observations return the last cached result.

    Network and console events are accumulated into the session history so that
    later debug endpoints can return the full history, not just the latest poll.
    """
    session = STATE.sessions.setdefault(session_id, {})
    if not fresh:
        last = session.get("last_observation")
        if last is not None:
            return last

    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})
    url = chrome_output.get("url", session.get("url", ""))
    title = chrome_output.get("title", session.get("title", ""))
    session["url"] = url
    session["title"] = title

    screenshot = capture_screenshot(session_id, fresh=fresh)
    a11y_summary = parse_snapshot(chrome_output.get("snapshot", ""))

    network_summary = None
    console_summary = None
    if include_network:
        network = run_chrome_agent(session_id, ["network"], timeout=10)
        if network.get("ok"):
            _merge_history(session, "network_history", network.get("requests", []), action_seq)
            network_summary = summarize_network(
                [entry["item"] for entry in session.get("network_history", [])],
                limit=max_debug_items,
            )
    if include_console:
        console = run_chrome_agent(session_id, ["console"], timeout=10)
        if console.get("ok"):
            _merge_history(session, "console_history", console.get("messages", []), action_seq)
            console_summary = summarize_console(
                [entry["item"] for entry in session.get("console_history", [])],
                limit=max_debug_items,
            )

    observation_seq = session.get("observation_seq", 0) + 1
    session["observation_seq"] = observation_seq

    observation = {
        "observation_id": f"obs-{session_id}-{observation_seq}",
        "action_seq": action_seq,
        "captured_at": now_iso(),
        "url": url,
        "title": title,
        "viewport": viewport,
        "loading_state": "idle",
        "screenshot": screenshot,
        "a11y_summary": a11y_summary,
        "network_summary": network_summary,
        "console_summary": console_summary,
    }
    session["last_observation"] = observation
    return observation


def _press_args(key: str) -> list[str]:
    """Map a press key to a chrome-agent invocation.

    Simple keys (e.g., ``Escape``, ``Enter``) are passed through to the native
    chrome-agent ``press`` command.  Combinations like ``ctrl+a`` or
    ``shift+enter`` are dispatched via a JavaScript ``KeyboardEvent`` so the
    sidecar does not depend on chrome-agent's key-combination syntax.
    """
    if "+" not in key:
        return ["press", key]

    parts = [part.strip().lower() for part in key.split("+")]
    modifier_aliases = {
        "ctrl": "ctrlKey",
        "control": "ctrlKey",
        "alt": "altKey",
        "shift": "shiftKey",
        "meta": "metaKey",
        "command": "metaKey",
        "cmd": "metaKey",
        "win": "metaKey",
    }
    key_aliases = {
        "enter": "Enter",
        "return": "Enter",
        "tab": "Tab",
        "escape": "Escape",
        "esc": "Escape",
        "space": " ",
        "spacebar": " ",
        "backspace": "Backspace",
        "delete": "Delete",
        "del": "Delete",
        "arrowup": "ArrowUp",
        "arrowdown": "ArrowDown",
        "arrowleft": "ArrowLeft",
        "arrowright": "ArrowRight",
        "home": "Home",
        "end": "End",
        "pageup": "PageUp",
        "pagedown": "PageDown",
    }
    modifiers = {"ctrlKey": False, "altKey": False, "shiftKey": False, "metaKey": False}
    keys: list[str] = []
    for part in parts:
        if part in modifier_aliases:
            modifiers[modifier_aliases[part]] = True
        elif part:
            keys.append(key_aliases.get(part, part))

    if not keys:
        return ["eval", "(() => { return 'Error: no key in combo'; })()"]

    resolved = keys[0]
    modifier_js = ", ".join(
        f"{name}: {str(value).lower()}" for name, value in modifiers.items()
    )
    script = (
        "(() => { const target = document.activeElement || document.body; "
        "const init = { key: " + json.dumps(resolved) + ", bubbles: true, cancelable: true, "
        + modifier_js + " }; "
        "['keydown', 'keypress', 'keyup'].forEach(type => target.dispatchEvent(new KeyboardEvent(type, init))); "
        "return " + json.dumps("dispatched " + key) + "; })()"
    )
    return ["eval", script]


def action_to_chrome_args(action: dict[str, Any]) -> list[str]:
    kind = action.get("kind")
    if kind == "click_xy":
        x = action.get("x", 0)
        y = action.get("y", 0)
        return ["click", "--xy", f"{x},{y}"]
    if kind == "click_selector":
        return ["click", "--selector", action["selector"]]
    if kind == "click_target_id":
        return ["click", action["target_id"]]
    if kind == "fill":
        return ["fill", "--selector", action["selector"], action["value"]]
    if kind == "type_text":
        return ["type", action["text"]]
    if kind == "press":
        return _press_args(action["key"])
    if kind == "scroll":
        dx = action.get("delta_x", 0)
        dy = action.get("delta_y", 0)
        # Use JS eval for arbitrary scroll; chrome-agent scroll is page/element oriented.
        return ["eval", f"window.scrollBy({dx},{dy}); true"]
    if kind == "get_element_value":
        selector = action["selector"]
        return ["eval", f"(() => {{ const el = document.querySelector({json.dumps(selector)}); if (!el) return 'Error: element not found'; return el.value !== undefined ? el.value : el.textContent; }})()"]
    if kind == "execute_javascript":
        expression = action["expression"]
        return ["eval", f"(() => {{ try {{ return ({expression}); }} catch (err) {{ return 'Error: ' + (err.message || err); }} }})()"]
    if kind == "wait":
        # chrome-agent wait expects a condition. Simple timeout wait is a no-op here.
        return ["eval", "true"]
    raise ValueError(f"unsupported action kind: {kind}")


def _extract_eval_result(result: dict[str, Any]) -> str | None:
    """Extract the string value returned by a chrome-agent eval action."""
    if not result.get("ok"):
        return None
    for key in ("result", "value"):
        if key in result:
            value = result[key]
            if value is None:
                return None
            if isinstance(value, (dict, list)):
                return json.dumps(value, ensure_ascii=False)
            return str(value)
    return None


class Handler(BaseHTTPRequestHandler):
    server_version = "oxide-browser-sidecar/0.2"

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/healthz":
            self._handle_healthz()
            return

        if parsed.path.endswith("/screenshot/latest"):
            session_id = session_id_from_path(parsed.path, "/screenshot/latest")
            if not self.ensure_auth() or not session_id:
                return
            query = parse_qs(parsed.query)
            format_value = query.get("format", ["metadata"])[0]
            redacted = query.get("redacted", ["false"])[0].lower() == "true"
            self._handle_screenshot_latest(session_id, format_value, redacted)
            return

        if parsed.path.endswith("/observe"):
            session_id = session_id_from_path(parsed.path, "/observe")
            if not self.ensure_auth() or not session_id:
                return
            query = parse_qs(parsed.query)
            include_network = query.get("include_network_summary", ["true"])[0].lower() != "false"
            include_console = query.get("include_console_summary", ["true"])[0].lower() != "false"
            fresh = query.get("fresh", ["false"])[0].lower() == "true"
            max_debug_items = int(query.get("max_debug_items", ["20"])[0] or 20)
            self._handle_observe(session_id, include_network, include_console, fresh, max_debug_items)
            return

        if parsed.path.endswith("/debug/network"):
            session_id = session_id_from_path(parsed.path, "/debug/network")
            if not self.ensure_auth() or not session_id:
                return
            query = parse_qs(parsed.query)
            self._handle_debug_network(session_id, query)
            return

        if parsed.path.endswith("/debug/console"):
            session_id = session_id_from_path(parsed.path, "/debug/console")
            if not self.ensure_auth() or not session_id:
                return
            query = parse_qs(parsed.query)
            self._handle_debug_console(session_id, query)
            return

        self.error_json(HTTPStatus.NOT_FOUND, "not_found", "endpoint not found")

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/sessions":
            if not self.ensure_auth():
                return
            self._handle_create_session()
            return

        if parsed.path.endswith("/goto"):
            session_id = session_id_from_path(parsed.path, "/goto")
            if not self.ensure_auth() or not session_id:
                return
            self._handle_goto(session_id)
            return

        if parsed.path.endswith("/action"):
            session_id = session_id_from_path(parsed.path, "/action")
            if not self.ensure_auth() or not session_id:
                return
            self._handle_action(session_id)
            return

        self.error_json(HTTPStatus.NOT_FOUND, "not_found", "endpoint not found")

    def do_DELETE(self) -> None:
        parsed = urlparse(self.path)
        session_id = session_id_from_path(parsed.path, "")
        if not self.ensure_auth() or not session_id:
            return
        self._handle_close_session(session_id)

    def _handle_healthz(self) -> None:
        available = chrome_agent_available()
        status = run_chrome_agent("default", ["status"], timeout=10) if available else {"ok": False}
        self.write_json(
            HTTPStatus.OK if available else HTTPStatus.SERVICE_UNAVAILABLE,
            {
                "ok": available and status.get("ok", False),
                "chrome_agent_available": available,
                "chrome_agent_status": status.get("daemon") if isinstance(status, dict) else None,
                "auth_configured": bool(STATE.token),
                "artifact_dir": str(STATE.artifact_dir),
                "profile_dir": str(STATE.profile_dir),
            },
        )

    def _handle_create_session(self) -> None:
        body = self.read_json()
        task_id = str(body.get("task_id") or "browser-task")
        session_id = f"br-{uuid.uuid4().hex[:12]}"
        viewport = body.get("viewport") or {"width": 1365, "height": 768, "device_scale_factor": 1.0}
        start_url = str(body.get("start_url") or "about:blank")

        STATE.sessions[session_id] = {
            "task_id": task_id,
            "viewport": viewport,
            "url": start_url,
            "title": "",
            "action_seq": 0,
            "screenshot_seq": 0,
            "observation_seq": 0,
            "last_screenshot": None,
            "last_observation": None,
            "network_history": [],
            "console_history": [],
        }

        result = run_chrome_agent(session_id, ["goto", start_url], timeout=60)
        if not result.get("ok"):
            STATE.sessions.pop(session_id, None)
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": result.get("error"),
            })
            return

        STATE.sessions[session_id]["url"] = result.get("url", start_url)
        STATE.sessions[session_id]["title"] = result.get("title", "")

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "browser": {
                "browser_id": "chrome-agent",
                "page_id": "default",
                "cdp_connected": True,
            },
            "viewport": viewport,
            "artifact_root": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/",
        })

    def _handle_goto(self, session_id: str) -> None:
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        body = self.read_json()
        url = str(body.get("url", ""))
        if not url:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "invalid_action",
                    "message": "goto requires a url",
                    "retryable": False,
                    "hint": "provide a non-empty url",
                },
            })
            return

        result = run_chrome_agent(session_id, ["goto", url], timeout=60)
        if not result.get("ok"):
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "navigation": {"url": url, "final_url": url, "status": "blocked", "http_status": None, "redirect_count": 0},
                "observation": None,
                "error": result.get("error"),
            })
            return

        action_seq = session.get("action_seq", 0)
        # chrome-agent goto returns navigation metadata; use a separate inspect
        # to get the post-navigation snapshot and a11y tree.
        inspect_result = run_chrome_agent(session_id, ["inspect"], timeout=15)
        chrome_output = inspect_result if inspect_result.get("ok") else result
        observation = build_observation(session_id, chrome_output, action_seq=action_seq, max_debug_items=20)
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "navigation": {
                "url": url,
                "final_url": result.get("url", url),
                "status": "loaded",
                "http_status": None,
                "redirect_count": 0,
            },
            "observation": observation,
            "error": None,
        })

    def _handle_observe(
        self,
        session_id: str,
        include_network: bool,
        include_console: bool,
        fresh: bool,
        max_debug_items: int,
    ) -> None:
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        action_seq = session.get("action_seq", 0)
        result = run_chrome_agent(session_id, ["inspect"], timeout=15)
        if not result.get("ok"):
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": result.get("error"),
            })
            return

        observation = build_observation(
            session_id,
            result,
            action_seq=action_seq,
            include_network=include_network,
            include_console=include_console,
            fresh=fresh,
            max_debug_items=max_debug_items,
        )
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "observation": observation,
            "error": None,
        })

    def _handle_action(self, session_id: str) -> None:
        body = self.read_json()
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        action = body.get("action", {})
        action_seq = int(body.get("action_seq", session.get("action_seq", 0) + 1))
        session["action_seq"] = action_seq
        capture_after = bool(body.get("capture_after", True))
        wait_for_stability = bool(body.get("wait_for_stability", True))

        try:
            args = action_to_chrome_args(action)
        except ValueError as exc:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "action_result": {
                    "action_seq": action_seq,
                    "kind": action.get("kind", "unknown"),
                    "status": "failed",
                    "duration_ms": 0,
                    "technical_success": False,
                    "hint": str(exc),
                },
                "post_observation": None,
                "error": {
                    "code": "invalid_action",
                    "message": str(exc),
                    "retryable": False,
                    "hint": "use a supported action kind",
                },
            })
            return

        # chrome-agent actions that change the page should also inspect after.
        mutating_kinds = {"click_xy", "click_selector", "click_target_id", "fill", "type_text", "press", "scroll"}
        if action.get("kind") in mutating_kinds:
            args.append("--inspect")

        started = time.time()
        result = run_chrome_agent(session_id, args, timeout=60)
        duration_ms = int((time.time() - started) * 1000)
        success = result.get("ok", False)

        post_observation = None
        if success and capture_after:
            # wait_for_stability is accepted by the contract. chrome-agent's --inspect
            # already provides a post-action snapshot for mutating kinds; dedicated
            # stability polling is a future improvement.
            post_observation = build_observation(session_id, result, action_seq=action_seq, max_debug_items=20)

        result_value = None
        if success and action.get("kind") in ("get_element_value", "execute_javascript"):
            result_value = _extract_eval_result(result)

        eval_error = (
            result_value is not None
            and isinstance(result_value, str)
            and result_value.startswith("Error:")
        )
        if success and eval_error:
            success = False

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": success,
            "action_result": {
                "action_seq": action_seq,
                "kind": action.get("kind", "unknown"),
                "status": "failed" if not success else "executed",
                "duration_ms": duration_ms,
                "technical_success": success,
                "hint": (
                    result_value
                    if (not success and eval_error)
                    else result.get("error", {}).get("hint") if isinstance(result.get("error"), dict) else ""
                ),
                "result": None if not success else result_value,
            },
            "post_observation": post_observation,
            "error": result.get("error") if not success else None,
        })

    def _handle_screenshot_latest(self, session_id: str, format_value: str, redacted: bool) -> None:
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        screenshot = session.get("last_screenshot")
        if screenshot is None:
            screenshot = capture_screenshot(session_id, fresh=True)
        if redacted:
            screenshot["redacted"] = True

        if format_value == "binary":
            artifact_dir = session_artifact_dir(session.get("task_id", "browser-task"), session_id)
            path = artifact_dir / "latest.png"
            data = path.read_bytes() if path.exists() else ONE_PIXEL_PNG
            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "screenshot": screenshot,
            "error": None,
        })

    def _handle_debug_network(self, session_id: str, query: dict[str, list[str]]) -> None:
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        since_action_seq = int(query.get("since_action_seq", ["0"])[0] or 0)
        filter_value = query.get("filter", ["failed"])[0].lower()
        level = query.get("level", ["summary"])[0].lower()
        include_bodies = query.get("include_bodies", ["false"])[0].lower() == "true"
        limit = int(query.get("limit", ["20"])[0] or 20)

        payload = build_network_debug_payload(
            session.get("network_history", []),
            since_action_seq=since_action_seq,
            filter_value=filter_value,
            level=level,
            include_bodies=include_bodies,
            limit=limit,
        )
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "network": payload,
            "error": None,
        })

    def _handle_debug_console(self, session_id: str, query: dict[str, list[str]]) -> None:
        session = STATE.sessions.get(session_id)
        if session is None:
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "not_found",
                    "message": "browser session not found",
                    "retryable": False,
                    "hint": "start a new session",
                },
            })
            return

        since_action_seq = int(query.get("since_action_seq", ["0"])[0] or 0)
        level = query.get("level", ["summary"])[0].lower()
        min_level = query.get("min_level", ["error"])[0].lower()
        limit = int(query.get("limit", ["20"])[0] or 20)

        payload = build_console_debug_payload(
            session.get("console_history", []),
            since_action_seq=since_action_seq,
            level=level,
            min_level=min_level,
            limit=limit,
        )
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "console": payload,
            "error": None,
        })

    def _handle_close_session(self, session_id: str) -> None:
        body = self.read_json(default={})
        purge = bool(body.get("purge_profile", True))
        keep_artifacts = bool(body.get("keep_artifacts", True))

        session = STATE.sessions.pop(session_id, None)
        args = ["close"]
        if purge:
            args.append("--purge")
        result = run_chrome_agent(session_id, args, timeout=15)

        if not keep_artifacts and session is not None:
            artifact_dir = session_artifact_dir(session.get("task_id", "browser-task"), session_id)
            shutil.rmtree(artifact_dir, ignore_errors=True)

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": result.get("ok", True),
            "closed": True,
            "profile_purged": purge,
            "artifacts_kept": keep_artifacts,
            "error": result.get("error") if not result.get("ok") else None,
        })

    def ensure_auth(self) -> bool:
        if not STATE.token:
            self.error_json(
                HTTPStatus.SERVICE_UNAVAILABLE,
                "missing_token",
                "BROWSER_AGENT_SIDECAR_TOKEN is required before browser API calls are enabled",
            )
            return False
        header = self.headers.get("Authorization", "")
        if header != f"Bearer {STATE.token}":
            self.error_json(HTTPStatus.UNAUTHORIZED, "unauthorized", "invalid bearer token")
            return False
        return True

    def read_json(self, default: Any | None = None) -> Any:
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length == 0:
            return {} if default is None else default
        return json.loads(self.rfile.read(length).decode("utf-8"))

    def write_json(self, status: HTTPStatus, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def error_json(self, status: HTTPStatus, code: str, message: str) -> None:
        self.write_json(status, {"code": code, "message": message, "retryable": False})

    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"sidecar {self.address_string()} {fmt % args}", flush=True)


def self_test() -> int:
    if not chrome_agent_available():
        print("chrome-agent-sidecar self-test: chrome-agent binary not found", file=sys.stderr)
        return 1
    status = run_chrome_agent("self-test", ["status"], timeout=10)
    if not status.get("ok"):
        print(f"chrome-agent-sidecar self-test: status check failed: {status}", file=sys.stderr)
        return 1
    print("chrome-agent-sidecar self-test ok")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    STATE.reset()
    server = ThreadingHTTPServer((STATE.addr, STATE.port), Handler)

    def shutdown(_signum: int, _frame: Any) -> None:
        server.shutdown()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    print(f"chrome-agent-sidecar listening on {STATE.addr}:{STATE.port}", flush=True)
    try:
        server.serve_forever()
    finally:
        for session_id in list(STATE.sessions.keys()):
            run_chrome_agent(session_id, ["close", "--purge"], timeout=15)
        STATE.sessions.clear()
    return 0


if __name__ == "__main__":
    sys.exit(main())
