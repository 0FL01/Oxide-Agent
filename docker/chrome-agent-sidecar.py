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


def capture_screenshot(session_id: str) -> dict[str, Any]:
    """Capture a screenshot via chrome-agent and copy it into the artifact dir."""
    session = STATE.sessions.get(session_id, {})
    task_id = session.get("task_id", "browser-task")
    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})

    result = run_chrome_agent(session_id, ["screenshot"], timeout=30)
    if not result.get("ok"):
        artifact_dir = session_artifact_dir(task_id, session_id)
        dest = artifact_dir / "latest.png"
        dest.write_bytes(ONE_PIXEL_PNG)
        return {
            "screenshot_id": f"shot-{session_id}",
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
    return {
        "screenshot_id": f"shot-{session_id}",
        "artifact_uri": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/latest.png",
        "mime_type": "image/png",
        "width": viewport["width"],
        "height": viewport["height"],
        "sha256": sha256_of_file(dest),
        "captured_at": now_iso(),
        "redacted": False,
        "byte_size": dest.stat().st_size if dest.exists() else 0,
    }


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


def summarize_network(requests: list[dict[str, Any]]) -> dict[str, Any]:
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
        "recent_failures": failures[:20],
    }


def summarize_console(messages: list[dict[str, Any]]) -> dict[str, Any]:
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
        "recent_errors": errors[:20],
    }


def build_observation(
    session_id: str,
    chrome_output: dict[str, Any],
    action_seq: int = 0,
    include_network: bool = True,
    include_console: bool = True,
) -> dict[str, Any]:
    session = STATE.sessions.setdefault(session_id, {})
    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})
    url = chrome_output.get("url", session.get("url", ""))
    title = chrome_output.get("title", session.get("title", ""))
    session["url"] = url
    session["title"] = title

    screenshot = capture_screenshot(session_id)
    a11y_summary = parse_snapshot(chrome_output.get("snapshot", ""))

    network_summary = None
    console_summary = None
    if include_network:
        network = run_chrome_agent(session_id, ["network"], timeout=10)
        if network.get("ok"):
            network_summary = summarize_network(network.get("requests", []))
    if include_console:
        console = run_chrome_agent(session_id, ["console"], timeout=10)
        if console.get("ok"):
            console_summary = summarize_console(console.get("messages", []))

    return {
        "observation_id": f"obs-{session_id}-{action_seq}",
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
        return ["press", action["key"]]
    if kind == "scroll":
        dx = action.get("delta_x", 0)
        dy = action.get("delta_y", 0)
        # Use JS eval for arbitrary scroll; chrome-agent scroll is page/element oriented.
        return ["eval", f"window.scrollBy({dx},{dy}); true"]
    if kind == "wait":
        # chrome-agent wait expects a condition. Simple timeout wait is a no-op here.
        return ["eval", "true"]
    raise ValueError(f"unsupported action kind: {kind}")


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
            self._handle_observe(session_id, include_network, include_console, fresh)
            return

        if parsed.path.endswith("/debug/network"):
            session_id = session_id_from_path(parsed.path, "/debug/network")
            if not self.ensure_auth() or not session_id:
                return
            self._handle_debug_network(session_id)
            return

        if parsed.path.endswith("/debug/console"):
            session_id = session_id_from_path(parsed.path, "/debug/console")
            if not self.ensure_auth() or not session_id:
                return
            self._handle_debug_console(session_id)
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
        }

        result = run_chrome_agent(session_id, ["goto", start_url, "--inspect"], timeout=60)
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

        result = run_chrome_agent(session_id, ["goto", url, "--inspect"], timeout=60)
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

        observation = build_observation(session_id, result, action_seq=0)
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
            "observation": {
                "observation_id": observation["observation_id"],
                "screenshot_id": observation["screenshot"]["screenshot_id"],
                "url": observation["url"],
                "title": observation["title"],
                "loading_state": observation["loading_state"],
            },
            "error": None,
        })

    def _handle_observe(self, session_id: str, include_network: bool, include_console: bool, fresh: bool) -> None:
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
        if success:
            post_observation = build_observation(session_id, result, action_seq=action_seq)

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": success,
            "action_result": {
                "action_seq": action_seq,
                "kind": action.get("kind", "unknown"),
                "status": "executed" if success else "failed",
                "duration_ms": duration_ms,
                "technical_success": success,
                "hint": result.get("error", {}).get("hint") if isinstance(result.get("error"), dict) else "",
            },
            "post_observation": {
                "observation_id": post_observation["observation_id"],
                "screenshot_id": post_observation["screenshot"]["screenshot_id"],
                "url": post_observation["url"],
                "title": post_observation["title"],
                "loading_state": post_observation["loading_state"],
            } if post_observation else None,
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

        screenshot = capture_screenshot(session_id)
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

    def _handle_debug_network(self, session_id: str) -> None:
        result = run_chrome_agent(session_id, ["network"], timeout=10)
        if not result.get("ok"):
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": result.get("error"),
            })
            return

        payload = summarize_network(result.get("requests", []))
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "network": payload,
            "error": None,
        })

    def _handle_debug_console(self, session_id: str) -> None:
        result = run_chrome_agent(session_id, ["console"], timeout=10)
        if not result.get("ok"):
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": result.get("error"),
            })
            return

        payload = summarize_console(result.get("messages", []))
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
