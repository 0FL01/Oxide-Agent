#!/usr/bin/env python3
"""Minimal Browser Live sidecar wrapper for Docker Compose smoke.

The wrapper owns a headless Chromium process and exposes the stable health and
session lifecycle contract that Oxide needs for deployment validation. Full CDP
action execution remains in the Browser Live sidecar contract implemented in
Oxide; this wrapper deliberately avoids manual browser control surfaces.
"""

from __future__ import annotations

import base64
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
        self.chrome_bin = os.getenv("CHROME_BIN", "/usr/bin/chromium")
        self.chrome_agent_bin = shutil.which("chrome-agent") or ""
        self.chrome_port = int(os.getenv("CHROME_REMOTE_DEBUGGING_PORT", "9222"))
        self.sessions: dict[str, dict[str, Any]] = {}
        self.chrome: subprocess.Popen[str] | None = None
        self.chrome_profile: Path | None = None

    def start(self) -> None:
        self.artifact_dir.mkdir(parents=True, exist_ok=True)
        self.profile_dir.mkdir(parents=True, exist_ok=True)
        for old_profile in self.profile_dir.glob("sidecar-chromium*"):
            shutil.rmtree(old_profile, ignore_errors=True)
        chrome_profile = self.profile_dir / f"sidecar-chromium-{os.getpid()}"
        self.chrome_profile = chrome_profile
        chrome_profile.mkdir(parents=True, exist_ok=True)
        args = [
            self.chrome_bin,
            "--headless=new",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--no-sandbox",
            "--disable-setuid-sandbox",
            "--remote-debugging-address=127.0.0.1",
            f"--remote-debugging-port={self.chrome_port}",
            f"--user-data-dir={chrome_profile}",
            "about:blank",
        ]
        self.chrome = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def stop(self) -> None:
        if self.chrome and self.chrome.poll() is None:
            self.chrome.terminate()
            try:
                self.chrome.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.chrome.kill()
        if self.chrome_profile:
            shutil.rmtree(self.chrome_profile, ignore_errors=True)
        cleanup_profiles(self.profile_dir)

    def chrome_alive(self) -> bool:
        return self.chrome is not None and self.chrome.poll() is None


STATE = SidecarState()


class Handler(BaseHTTPRequestHandler):
    server_version = "oxide-browser-sidecar/0.1"

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/healthz":
            chrome_alive = STATE.chrome_alive()
            self.write_json(
                HTTPStatus.OK if chrome_alive else HTTPStatus.SERVICE_UNAVAILABLE,
                {
                    "ok": chrome_alive,
                    "chrome_alive": chrome_alive,
                    "chrome_agent_available": bool(STATE.chrome_agent_bin),
                    "chrome_cdp_host_exposed": False,
                    "auth_configured": bool(STATE.token),
                    "artifact_dir": str(STATE.artifact_dir),
                    "profile_dir": str(STATE.profile_dir),
                },
            )
            return
        if parsed.path.endswith("/screenshot/latest"):
            session_id = session_id_from_path(parsed.path, "/screenshot/latest")
            if not self.ensure_auth() or not session_id:
                return
            session = STATE.sessions.get(session_id)
            if not session:
                self.error_json(HTTPStatus.NOT_FOUND, "not_found", "browser session not found")
                return
            query = parse_qs(parsed.query)
            if query.get("format", ["metadata"])[0] == "binary":
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(ONE_PIXEL_PNG)))
                self.end_headers()
                self.wfile.write(ONE_PIXEL_PNG)
                return
            self.write_json(
                HTTPStatus.OK,
                {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": True,
                    "screenshot": session["screenshot"],
                },
            )
            return
        self.error_json(HTTPStatus.NOT_FOUND, "not_found", "endpoint not found")

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/sessions":
            if not self.ensure_auth():
                return
            body = self.read_json()
            task_id = str(body.get("task_id") or "browser-task")
            viewport = body.get("viewport") or {"width": 1365, "height": 768, "device_scale_factor": 1.0}
            session_id = f"br-{uuid.uuid4().hex[:12]}"
            profile_path = STATE.profile_dir / session_id
            artifact_path = STATE.artifact_dir / task_id / session_id
            profile_path.mkdir(parents=True, exist_ok=True)
            artifact_path.mkdir(parents=True, exist_ok=True)
            screenshot = screenshot_metadata(task_id, session_id, artifact_path, viewport)
            STATE.sessions[session_id] = {
                "task_id": task_id,
                "profile_path": str(profile_path),
                "artifact_path": str(artifact_path),
                "viewport": viewport,
                "screenshot": screenshot,
            }
            self.write_json(
                HTTPStatus.OK,
                {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": True,
                    "browser": {
                        "browser_id": "chromium-sidecar",
                        "page_id": "page-1",
                        "cdp_connected": STATE.chrome_alive(),
                    },
                    "viewport": viewport,
                    "artifact_root": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/",
                },
            )
            return
        self.error_json(HTTPStatus.NOT_FOUND, "not_found", "endpoint not found")

    def do_DELETE(self) -> None:
        parsed = urlparse(self.path)
        session_id = session_id_from_path(parsed.path, "")
        if not self.ensure_auth() or not session_id:
            return
        body = self.read_json(default={})
        session = STATE.sessions.pop(session_id, None)
        if not session:
            self.error_json(HTTPStatus.NOT_FOUND, "not_found", "browser session not found")
            return
        purge_profile = bool(body.get("purge_profile", True))
        if purge_profile:
            shutil.rmtree(session["profile_path"], ignore_errors=True)
        self.write_json(
            HTTPStatus.OK,
            {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": True,
                "closed": True,
                "profile_purged": purge_profile,
                "artifacts_kept": bool(body.get("keep_artifacts", True)),
            },
        )

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


def screenshot_metadata(task_id: str, session_id: str, artifact_path: Path, viewport: dict[str, Any]) -> dict[str, Any]:
    artifact_path.mkdir(parents=True, exist_ok=True)
    path = artifact_path / "latest.png"
    path.write_bytes(ONE_PIXEL_PNG)
    return {
        "screenshot_id": "shot-1",
        "artifact_uri": f"artifact://browser/{safe(task_id)}/{safe(session_id)}/latest.png",
        "mime_type": "image/png",
        "width": int(viewport.get("width", 1365)),
        "height": int(viewport.get("height", 768)),
        "sha256": "placeholder-sidecar-smoke-screenshot",
        "captured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "redacted": True,
    }


def request_id() -> str:
    return f"req-{uuid.uuid4().hex[:12]}"


def safe(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in "-_." else "-" for ch in value.strip()) or "unknown"


def session_id_from_path(path: str, suffix: str) -> str | None:
    prefix = "/sessions/"
    if not path.startswith(prefix) or (suffix and not path.endswith(suffix)):
        return None
    value = path[len(prefix) :]
    if suffix:
        value = value[: -len(suffix)]
    return value.strip("/") or None


def cleanup_profiles(root: Path) -> None:
    if not root.exists():
        return
    for child in root.iterdir():
        if not child.name.startswith("sidecar-chromium"):
            shutil.rmtree(child, ignore_errors=True)


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    STATE.start()
    server = ThreadingHTTPServer((STATE.addr, STATE.port), Handler)

    def shutdown(_signum: int, _frame: Any) -> None:
        server.shutdown()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    print(f"chrome-agent-sidecar listening on {STATE.addr}:{STATE.port}", flush=True)
    try:
        server.serve_forever()
    finally:
        STATE.stop()
    return 0


def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        profiles = root / "profiles"
        artifacts = root / "artifacts"
        (profiles / "session-a").mkdir(parents=True)
        (profiles / "sidecar-chromium-1").mkdir()
        cleanup_profiles(profiles)
        assert not (profiles / "session-a").exists()
        assert (profiles / "sidecar-chromium-1").exists()
        screenshot = screenshot_metadata(
            "smoke",
            "br-self-test",
            artifacts / "smoke" / "br-self-test",
            {"width": 1365, "height": 768},
        )
        assert screenshot["artifact_uri"].endswith("/latest.png")
        assert (artifacts / "smoke" / "br-self-test" / "latest.png").is_file()
    print("chrome-agent-sidecar self-test ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
