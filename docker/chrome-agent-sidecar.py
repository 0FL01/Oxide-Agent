#!/usr/bin/env python3
"""Browser Live sidecar: stateful HTTP adapter over chrome-agent pipe.

The sidecar exposes the stable REST contract that Oxide expects, authorizes
requests, and keeps one persistent `chrome-agent --json pipe` subprocess per
browser session. Each REST request is translated into a JSON-line command sent
to the pipe, and the JSON-line response is returned to the caller.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import uuid
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

import websockets


ONE_PIXEL_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC"
)

IDEMPOTENCY_KEY_HEADER = "Idempotency-Key"

# Short pause before draining the CDP listener queue so that any late
# ``Network.loadingFinished`` events from the just-completed action arrive.
CDP_DRAIN_DELAY_SECONDS = float(os.getenv("BROWSER_AGENT_CDP_DRAIN_DELAY_SECONDS", "0.2"))


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
        self.pipes: dict[str, ChromeAgentPipe] = {}
        self.artifact_dir.mkdir(parents=True, exist_ok=True)
        self.profile_dir.mkdir(parents=True, exist_ok=True)

    def reset(self) -> None:
        for pipe in list(self.pipes.values()):
            pipe.close(purge=True)
        self.pipes.clear()
        for child in self.profile_dir.iterdir():
            shutil.rmtree(child, ignore_errors=True)
        self.sessions.clear()


STATE = SidecarState()


def request_id() -> str:
    return f"req-{uuid.uuid4().hex[:12]}"


def safe(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in "-_" else "-" for ch in value.strip()) or "unknown"


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


class ChromeAgentPipe:
    """One persistent chrome-agent pipe per browser session.

    The pipe is synchronous request/response: each command line produces one
    response line.  Responses are queued by a reader thread and consumed by the
    sending thread, so HTTP request handlers can block on a single command
    without polling the pipe directly.
    """

    def __init__(self, session_id: str, task_id: str) -> None:
        self.session_id = session_id
        self.task_id = task_id
        self._proc: subprocess.Popen | None = None
        self._reader_thread: threading.Thread | None = None
        self._queue: queue.Queue[dict[str, Any]] = queue.Queue()
        self._cdp_listener: CDPListener | None = None
        self._closed = False
        self._start()

    def _start(self) -> None:
        cmd = ["chrome-agent", "--browser", self.session_id, "--json", "pipe"]
        env = os.environ.copy()
        env.setdefault("CHROME_BIN", STATE.chrome_bin)
        try:
            self._proc = subprocess.Popen(
                cmd,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=env,
            )
        except FileNotFoundError as exc:
            raise RuntimeError("chrome-agent binary not found") from exc
        self._reader_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._reader_thread.start()
        self._start_cdp_listener()

    def _start_cdp_listener(self) -> None:
        """Start a background CDP listener once the browser is reachable."""
        def worker() -> None:
            browser_ws_url: str | None = None
            for _ in range(20):
                browser_ws_url = _find_browser_ws_url(self.session_id)
                if browser_ws_url:
                    break
                time.sleep(0.5)
            if not browser_ws_url:
                print(f"CDP listener: failed to discover browser URL for {self.session_id}", file=sys.stderr)
                return
            listener = CDPListener(self.session_id, browser_ws_url)
            listener.start()
            self._cdp_listener = listener
        threading.Thread(target=worker, daemon=True).start()

    def _read_stdout(self) -> None:
        try:
            proc = self._proc
            if proc is None or proc.stdout is None:
                return
            for line in proc.stdout:
                line = line.strip()
                if not line:
                    continue
                try:
                    data = json.loads(line)
                except json.JSONDecodeError:
                    continue
                self._queue.put(data)
        except Exception:
            pass

    def _drain_queue(self) -> None:
        try:
            while True:
                self._queue.get_nowait()
        except queue.Empty:
            pass

    def send(self, cmd_obj: dict[str, Any], timeout: int = 60) -> dict[str, Any]:
        if self._closed or self._proc is None or self._proc.poll() is not None:
            return {
                "ok": False,
                "error": {
                    "code": "sidecar_not_ready",
                    "message": "chrome-agent pipe is not running",
                    "retryable": True,
                    "hint": "start a new session",
                },
            }
        # Discard stale responses from a previous timed-out command.
        self._drain_queue()
        line = json.dumps(cmd_obj, separators=(",", ":")) + "\n"
        try:
            self._proc.stdin.write(line)
            self._proc.stdin.flush()
        except BrokenPipeError:
            return {
                "ok": False,
                "error": {
                    "code": "sidecar_pipe_broken",
                    "message": "chrome-agent pipe stdin closed",
                    "retryable": True,
                    "hint": "start a new session",
                },
            }
        try:
            return self._queue.get(timeout=timeout)
        except queue.Empty:
            return {
                "ok": False,
                "error": {
                    "code": "timeout",
                    "message": f"chrome-agent pipe timed out after {timeout}s",
                    "retryable": True,
                    "hint": "retry after observing browser state",
                },
            }

    def close(self, purge: bool = True) -> dict[str, Any]:
        self._closed = True
        listener = self._cdp_listener
        self._cdp_listener = None
        if listener is not None:
            try:
                listener.close()
            except Exception:
                pass
        # Ask chrome-agent to close the browser and purge the profile via the
        # standalone CLI, then terminate the pipe process itself.
        result = {"ok": True, "closed": True}
        if purge:
            try:
                result = run_chrome_agent(self.session_id, ["close", "--purge"], timeout=15)
            except Exception:
                pass
        if self._proc is not None:
            try:
                self._proc.stdin.close()
            except Exception:
                pass
            try:
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    self._proc.kill()
                    self._proc.wait(timeout=5)
                except Exception:
                    pass
            except Exception:
                pass
        return result if isinstance(result, dict) else {"ok": True, "closed": True}


def run_chrome_agent(browser: str, args: list[str], timeout: int = 60) -> dict[str, Any]:
    """Run a one-off chrome-agent CLI invocation (health checks, cleanup)."""
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

    if data.get("ok") is False and isinstance(data.get("error"), str):
        data["error"] = {
            "code": "chrome_agent_error",
            "message": data["error"],
            "retryable": False,
            "hint": data.get("hint", "") or "",
        }
    return data


def restart_pipe_with_fresh_browser(
    session_id: str,
    task_id: str,
    old_pipe: ChromeAgentPipe,
) -> tuple[ChromeAgentPipe | None, dict[str, Any] | None]:
    """Replace a pipe with a new browser process while preserving profile data.

    `force_reload` is a sidecar-owned freshness contract.  Closing the managed
    browser (without `--purge`) drops the page's JS heap and in-memory SPA state
    while preserving cookies/local storage, then a new pipe can navigate to the
    exact target URL including its hash.
    """
    close_result = run_chrome_agent(session_id, ["close"], timeout=15)
    try:
        old_pipe.close(purge=False)
    except Exception:
        pass

    if not close_result.get("ok"):
        error = close_result.get("error")
        if not isinstance(error, dict):
            error = {
                "code": "fresh_navigation_failed",
                "message": "failed to close browser for fresh navigation",
                "retryable": True,
                "hint": "start a new browser session",
            }
        return None, error

    try:
        return ChromeAgentPipe(session_id, task_id), None
    except Exception as exc:
        return None, {
            "code": "sidecar_not_ready",
            "message": f"failed to start fresh chrome-agent pipe: {exc}",
            "retryable": True,
            "hint": "start a new browser session",
        }


def _is_same_origin_path_hash_navigation(current_url: str, target_url: str) -> bool:
    """Return true for same-document hash navigation handled without reload."""
    parsed_current = urlparse(current_url)
    parsed_target = urlparse(target_url)
    return (
        bool(parsed_current.scheme)
        and parsed_current.scheme == parsed_target.scheme
        and parsed_current.netloc == parsed_target.netloc
        and parsed_current.path == parsed_target.path
        and (parsed_current.fragment != parsed_target.fragment or bool(parsed_target.fragment))
    )


def chrome_agent_available() -> bool:
    return shutil.which("chrome-agent") is not None


def _find_browser_ws_url(session_id: str) -> str | None:
    """Return the browser-level CDP WebSocket URL from chrome-agent status."""
    status = run_chrome_agent(session_id, ["status"], timeout=5)
    if not status.get("ok"):
        return None
    for browser in status.get("browsers", []):
        if browser.get("name") == session_id:
            return browser.get("ws")
    return None


def _find_page_ws_url(browser_ws_url: str) -> str | None:
    """Discover the first page target WS URL from the browser HTTP endpoint."""
    try:
        parsed = urlparse(browser_ws_url)
        port = parsed.port or 9222
        host = parsed.hostname or "127.0.0.1"
        with urllib.request.urlopen(f"http://{host}:{port}/json/list", timeout=10) as resp:
            targets = json.loads(resp.read())
        for target in targets:
            if target.get("type") == "page":
                return target.get("webSocketDebuggerUrl")
    except Exception:
        return None
    return None


def _resource_type_from_cdp(type_name: str) -> str:
    """Map CDP request type to the sidecar NetworkItem resource_type."""
    value = (type_name or "").lower()
    if value in ("xhr", "fetch"):
        return "xhr"
    if value in ("script", "javascript"):
        return "script"
    if value in ("stylesheet", "css"):
        return "stylesheet"
    if value in ("image", "png", "jpeg", "jpg", "webp", "gif", "svg"):
        return "image"
    if value in ("document", "html"):
        return "document"
    return value or "other"


MAX_BODY_CHARS = 4096


def _should_capture_body(entry: dict[str, Any]) -> bool:
    """Decide whether to fetch the response body for a completed request."""
    rt = (entry.get("resource_type") or "").lower()
    if rt in ("xhr", "fetch"):
        return True
    status = entry.get("status")
    if isinstance(status, int) and status >= 400:
        return True
    return False


def _decode_response_body(result: dict[str, Any] | None) -> str | None:
    """Decode a CDP Network.getResponseBody result into a UTF-8 string."""
    if not result:
        return None
    body = result.get("body", "")
    if not body:
        return None
    if result.get("base64Encoded"):
        try:
            body = base64.b64decode(body, validate=False).decode("utf-8", errors="replace")
        except Exception:
            return None
    return body[:MAX_BODY_CHARS]


# Network/console noise that should never reach the agent's summary.
NOISE_URL_SUFFIXES = ("/favicon.ico",)


def _is_noise_event(event: dict[str, Any]) -> bool:
    """Return True for network/console events that are irrelevant to the agent."""
    if event.get("type") == "console":
        text = event.get("text", "")
        return "favicon.ico" in text and event.get("level", "info") in ("info", "warning", "verbose")
    url = event.get("url", "")
    if any(url.endswith(suffix) for suffix in NOISE_URL_SUFFIXES):
        status = event.get("status")
        if status is None or (isinstance(status, int) and status == 404):
            return True
    return False


class CDPListener:
    """Continuous CDP listener for a single browser session.

    Runs in a background thread with its own asyncio event loop. It connects to
    the page target CDP WebSocket, enables Network/Log/Runtime domains, and
    queues completed network requests and console entries for the sidecar to
    drain when it builds an observation.
    """

    def __init__(self, session_id: str, browser_ws_url: str) -> None:
        self.session_id = session_id
        self.browser_ws_url = browser_ws_url
        self._page_ws_url: str | None = None
        self._ws = None
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._run_loop, daemon=True)
        self._queue: queue.Queue[dict[str, Any]] = queue.Queue()
        self._pending_requests: dict[str, dict[str, Any]] = {}
        self._pending_lock = threading.Lock()
        self._cmd_futures: dict[int, asyncio.Future] = {}
        self._cmd_lock = threading.Lock()
        self._closed = False
        self._connected = threading.Event()
        self._next_id = 1

    def start(self) -> None:
        self._thread.start()

    def wait_connected(self, timeout: float = 5.0) -> bool:
        return self._connected.wait(timeout)

    def close(self) -> None:
        self._closed = True
        if self._ws is not None:
            try:
                asyncio.run_coroutine_threadsafe(self._ws.close(), self._loop).result(timeout=2)
            except Exception:
                pass
        if self._loop.is_running():
            try:
                self._loop.call_soon_threadsafe(self._loop.stop)
            except Exception:
                pass
        if self._thread.is_alive():
            self._thread.join(timeout=5)

    def drain_events(self) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        try:
            while True:
                events.append(self._queue.get_nowait())
        except queue.Empty:
            pass
        return events

    def wait_for_network_idle(self, timeout: float = 2.0, idle_ms: int = 100) -> None:
        """Wait until no network requests are pending or timeout expires."""
        if not self._connected.is_set():
            return
        deadline = time.time() + timeout
        last_pending = None
        last_change = time.time()
        while time.time() < deadline:
            with self._pending_lock:
                pending = len(self._pending_requests)
            if pending == 0:
                break
            if pending != last_pending:
                last_pending = pending
                last_change = time.time()
            if (time.time() - last_change) * 1000 >= idle_ms:
                # Requests have been stuck for idle_ms; stop waiting.
                break
            time.sleep(0.05)

    def _run_loop(self) -> None:
        asyncio.set_event_loop(self._loop)
        self._loop.run_until_complete(self._listen())

    async def _send(self, ws: websockets.WebSocketClientProtocol, method: str, params: dict[str, Any] | None = None) -> int:
        cmd_id = self._next_id
        self._next_id += 1
        msg = {"id": cmd_id, "method": method, "params": params or {}}
        await ws.send(json.dumps(msg))
        return cmd_id

    async def _send_and_wait(
        self,
        ws: websockets.WebSocketClientProtocol,
        method: str,
        params: dict[str, Any] | None = None,
        timeout: float = 2.0,
    ) -> dict[str, Any] | None:
        cmd_id = await self._send(ws, method, params)
        loop = asyncio.get_event_loop()
        future = loop.create_future()
        with self._cmd_lock:
            self._cmd_futures[cmd_id] = future
        try:
            return await asyncio.wait_for(future, timeout=timeout)
        except asyncio.TimeoutError:
            with self._cmd_lock:
                self._cmd_futures.pop(cmd_id, None)
            return None

    async def _listen(self) -> None:
        try:
            self._page_ws_url = await self._loop.run_in_executor(
                None, _find_page_ws_url, self.browser_ws_url
            )
            if not self._page_ws_url:
                print(f"CDP listener: no page target for {self.session_id}", file=sys.stderr)
                return
            async with websockets.connect(self._page_ws_url) as ws:
                self._ws = ws
                await self._send(ws, "Network.enable", {})
                await self._send(ws, "Log.enable", {})
                await self._send(ws, "Runtime.enable", {})
                await self._send(ws, "Page.enable", {})
                self._connected.set()
                async for message in ws:
                    try:
                        data = json.loads(message)
                    except json.JSONDecodeError:
                        continue
                    self._on_message(data)
        except Exception as exc:
            if not self._closed:
                print(f"CDP listener error for {self.session_id}: {exc}", file=sys.stderr)

    def _on_message(self, data: dict[str, Any]) -> None:
        if "id" in data:
            with self._cmd_lock:
                future = self._cmd_futures.pop(data["id"], None)
            if future is not None:
                future.set_result(data.get("result"))
                return
        method = data.get("method")
        if not method:
            return
        params = data.get("params", {})
        if method == "Network.requestWillBeSent":
            self._on_request_will_be_sent(params)
        elif method == "Network.responseReceived":
            self._on_response_received(params)
        elif method == "Network.loadingFinished":
            self._on_loading_finished(params)
        elif method == "Network.loadingFailed":
            self._on_loading_failed(params)
        elif method == "Log.entryAdded":
            self._on_log_entry(params)
        elif method == "Runtime.consoleAPICalled":
            self._on_console_api_called(params)
        elif method == "Page.frameNavigated":
            self._on_frame_navigated(params)

    def _on_frame_navigated(self, params: dict[str, Any]) -> None:
        frame = params.get("frame", {})
        if not frame:
            return
        # Only update the main frame's URL; ignore subframes.
        if frame.get("parentId"):
            return
        url = frame.get("url", "")
        if not url:
            return
        session = STATE.sessions.get(self.session_id)
        if session is None:
            return
        # Avoid storing internal/blank pages as the session URL.
        if url.startswith("chrome-") or url == "about:blank":
            return
        session["url"] = url
        title = frame.get("name") or frame.get("title") or session.get("title", "")
        if title:
            session["title"] = title

    def _on_request_will_be_sent(self, params: dict[str, Any]) -> None:
        rid = params.get("requestId")
        req = params.get("request", {})
        if not rid:
            return
        with self._pending_lock:
            self._pending_requests[rid] = {
                "requestId": rid,
                "timestamp": params.get("timestamp"),
                "method": req.get("method", "GET"),
                "url": req.get("url", ""),
                "resource_type": _resource_type_from_cdp(params.get("type", "")),
                "status": None,
                "error_text": None,
                "body": None,
            }

    def _on_response_received(self, params: dict[str, Any]) -> None:
        rid = params.get("requestId")
        with self._pending_lock:
            entry = self._pending_requests.get(rid)
            if not entry:
                return
            response = params.get("response", {})
            entry["status"] = response.get("status")
            entry["url"] = entry.get("url") or response.get("url")

    def _on_loading_finished(self, params: dict[str, Any]) -> None:
        rid = params.get("requestId")
        with self._pending_lock:
            entry = self._pending_requests.pop(rid, None)
        if not entry:
            return
        if _should_capture_body(entry):
            try:
                asyncio.run_coroutine_threadsafe(self._fetch_body_and_queue(entry), self._loop)
                return
            except Exception:
                pass
        self._queue.put(entry)

    def _on_loading_failed(self, params: dict[str, Any]) -> None:
        rid = params.get("requestId")
        with self._pending_lock:
            entry = self._pending_requests.pop(rid, None)
        if not entry:
            return
        entry["error_text"] = params.get("errorText") or params.get("type")
        if _should_capture_body(entry):
            try:
                asyncio.run_coroutine_threadsafe(self._fetch_body_and_queue(entry), self._loop)
                return
            except Exception:
                pass
        self._queue.put(entry)

    def _on_log_entry(self, params: dict[str, Any]) -> None:
        entry = params.get("entry", {})
        self._queue.put({
            "type": "console",
            "level": entry.get("level", "info").lower(),
            "text": entry.get("text", ""),
            "source": entry.get("source"),
            "line": entry.get("lineNumber"),
            "timestamp": entry.get("timestamp"),
        })

    def _on_console_api_called(self, params: dict[str, Any]) -> None:
        level = params.get("type", "log").lower()
        if level == "warning":
            level = "warning"
        elif level == "error":
            level = "error"
        elif level in ("debug", "verbose"):
            level = "verbose"
        else:
            level = "info"
        text = " ".join(str(arg.get("value", arg)) for arg in params.get("args", []))
        self._queue.put({
            "type": "console",
            "level": level,
            "text": text,
            "source": "console-api",
            "line": None,
            "timestamp": params.get("timestamp"),
        })

    async def _fetch_body_and_queue(self, entry: dict[str, Any]) -> None:
        """Fetch the response body for a completed request and queue it."""
        rid = entry.get("requestId")
        ws = self._ws
        if ws is not None and self._connected.is_set() and rid:
            try:
                result = await self._send_and_wait(ws, "Network.getResponseBody", {"requestId": rid}, timeout=2.0)
                body = _decode_response_body(result)
                if body:
                    entry["body"] = body
            except Exception:
                pass
        self._queue.put(entry)


def get_pipe(session_id: str) -> ChromeAgentPipe:
    pipe = STATE.pipes.get(session_id)
    if pipe is None:
        raise KeyError(session_id)
    return pipe


def session_artifact_dir(task_id: str, session_id: str) -> Path:
    path = STATE.artifact_dir / safe(task_id) / safe(session_id)
    path.mkdir(parents=True, exist_ok=True)
    return path


def capture_screenshot(session_id: str, fresh: bool = True) -> dict[str, Any]:
    """Capture a screenshot via the chrome-agent pipe and copy it into the artifact dir."""
    session = STATE.sessions.get(session_id, {})
    task_id = session.get("task_id", "browser-task")
    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})

    if not fresh:
        last = session.get("last_screenshot")
        if last is not None:
            return last

    pipe = get_pipe(session_id)
    result = pipe.send({"cmd": "screenshot"}, timeout=30)
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
        uid = ""
        role = ""
        text = ""
        if " " in stripped:
            uid_part, rest = stripped.split(" ", 1)
            uid = uid_part.split("=", 1)[1] if "=" in uid_part else ""
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


DOM_SNAPSHOT_MAX_ELEMENTS = 150
DOM_SNAPSHOT_MAX_TEXT = 200
DOM_SNAPSHOT_MAX_ATTR = 512


DOM_SNAPSHOT_SCRIPT = """
(() => {
  const MAX = 150;
  const TEXT_MAX = 200;
  const ATTR_MAX = 512;
  function selectorHint(el) {
    const tag = el.tagName.toLowerCase();
    const parts = [tag];
    if (el.id) parts.push('#' + el.id);
    const testId = el.getAttribute('data-testid');
    if (testId) parts.push('[data-testid="' + String(testId).replace(/"/g, '\\"') + '"]');
    if (el.name) parts.push('[name="' + String(el.name).replace(/"/g, '\\"') + '"]');
    const cls = Array.from(el.classList).slice(0, 2).join('.');
    if (cls) parts.push('.' + cls);
    return parts.join('');
  }
  const candidates = new Set();
  const add = (el) => { if (el) candidates.add(el); };
  document.querySelectorAll('a, button, input, textarea, select, [role="button"], [role="link"], [data-clipboard-text]').forEach(add);
  document.querySelectorAll('[data-clipboard-text]').forEach(add);
  const seen = new Set();
  const result = [];
  for (const el of candidates) {
    if (seen.has(el)) continue;
    seen.add(el);
    if (result.length >= MAX) break;
    const tag = el.tagName.toLowerCase();
    const attrs = {};
    for (const name of el.getAttributeNames()) {
      if (name.startsWith('data-')) {
        attrs[name] = String(el.getAttribute(name) || '').slice(0, ATTR_MAX);
      }
    }
    let href = null;
    if (tag === 'a') {
      try {
        href = new URL(el.href, location.href).href;
      } catch (e) {
        href = el.getAttribute('href');
      }
    }
    let value = null;
    if ('value' in el) {
      value = el.value;
    }
    let text = (el.innerText || '').trim().slice(0, TEXT_MAX);
    if (!text) {
      text = el.getAttribute('aria-label') || el.getAttribute('title') || '';
    }
    result.push({ tag, selector: selectorHint(el), attributes: attrs, href, value, text });
  }
  return JSON.stringify(result);
})()
"""


def _dom_snapshot_error(
    code: str,
    message: str,
    hint: str | None = None,
    details: dict[str, Any] | None = None,
) -> dict[str, Any]:
    error: dict[str, Any] = {
        "code": code,
        "message": message,
        "retryable": True,
    }
    if hint:
        error["hint"] = hint
    if details is not None:
        error["details"] = details
    return error


def capture_dom_snapshot(pipe: ChromeAgentPipe | None) -> tuple[list[dict[str, Any]] | None, dict[str, Any] | None]:
    """Capture a compact DOM snapshot with resolved URLs and data-* attributes."""
    if pipe is None:
        return None, _dom_snapshot_error(
            "dom_snapshot_unavailable",
            "browser pipe is not available for DOM snapshot capture",
            "restart the browser session before retrying",
        )
    result = pipe.send({"cmd": "eval", "expression": DOM_SNAPSHOT_SCRIPT}, timeout=15)
    if not result.get("ok"):
        return None, _dom_snapshot_error(
            "dom_snapshot_failed",
            "chrome-agent failed to evaluate the DOM snapshot script",
            "inspect action_result and browser_debug output before retrying",
            {"error": result.get("error")},
        )
    raw = result.get("result") or result.get("value")
    if not raw or not isinstance(raw, str):
        return None, _dom_snapshot_error(
            "dom_snapshot_empty_result",
            "DOM snapshot script returned no JSON string",
            "retry after the page has finished rendering",
            {"result_type": type(raw).__name__},
        )
    try:
        parsed = json.loads(raw)
        if not isinstance(parsed, list):
            return None, _dom_snapshot_error(
                "dom_snapshot_invalid_shape",
                "DOM snapshot script returned JSON that is not an array",
                "inspect browser_debug output before retrying",
                {"parsed_type": type(parsed).__name__},
            )
        return parsed, None
    except json.JSONDecodeError as exc:
        return None, _dom_snapshot_error(
            "dom_snapshot_invalid_json",
            "DOM snapshot script returned invalid JSON",
            "inspect browser_debug output before retrying",
            {"error": str(exc)},
        )


def _resource_type_from_content_type(content_type: str | None) -> str:
    """Map chrome-agent live-network contentType to the sidecar resource_type."""
    value = (content_type or "").lower()
    if "xhr" in value or "fetch" in value:
        return "xhr"
    if "script" in value:
        return "script"
    if "stylesheet" in value or "css" in value:
        return "stylesheet"
    if "image" in value or value.endswith("png") or value.endswith("jpg") or value.endswith("jpeg"):
        return "image"
    if "document" in value or "html" in value:
        return "document"
    return value or "other"


def normalize_network_item(item: dict[str, Any]) -> dict[str, Any]:
    """Convert a chrome-agent live-network item into the canonical NetworkItem shape."""
    status = item.get("status")
    if not isinstance(status, int) or status == 0:
        status = None
    return {
        "timestamp": item.get("timestamp", now_iso()),
        "method": item.get("method", "GET"),
        "url_redacted": item.get("url", ""),
        "status": status,
        "resource_type": _resource_type_from_content_type(item.get("contentType")),
        "error_text": item.get("error_text"),
    }


def summarize_network(requests: list[dict[str, Any]], limit: int = 20) -> dict[str, Any]:
    """Map chrome-agent network requests to the expanded Rust NetworkSummary shape."""
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
        "request_count": len(requests),
        "recent_requests": requests[:limit],
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
        if _is_noise_event(item):
            continue
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
    items = [entry["item"] for entry in history if entry["action_seq"] >= since_action_seq]
    if filter_value == "failed":
        items = [
            item for item in items
            if (isinstance(item.get("status"), int) and item["status"] >= 400)
            or item.get("error_text")
        ]
    elif filter_value == "xhr":
        items = [item for item in items if "xhr" in (item.get("resource_type") or "").lower()]
    elif filter_value == "fetch":
        items = [item for item in items if "fetch" in (item.get("resource_type") or "").lower()]
    elif filter_value == "document":
        items = [item for item in items if (item.get("resource_type") or "").lower() == "document"]
    failures = [item for item in items if isinstance(item.get("status"), int) and item["status"] >= 400]
    items = items[-limit:]
    # Body capture is optional and expensive; only expose it when explicitly requested.
    if include_bodies:
        items = [_network_item_with_body(item) for item in items]
    else:
        items = [_network_item_without_body(item) for item in items]
    return {
        "failed_count": len(failures),
        "items": items,
        "artifact_uri": None,
    }


def _network_item_without_body(item: dict[str, Any]) -> dict[str, Any]:
    """Return a copy of a network item without the response body field."""
    copy = dict(item)
    copy.pop("body", None)
    return copy


def _network_item_with_body(item: dict[str, Any]) -> dict[str, Any]:
    """Return a copy of a network item with body only for XHR/fetch/failed requests."""
    copy = dict(item)
    rt = (copy.get("resource_type") or "").lower()
    status = copy.get("status")
    if rt in ("xhr", "fetch") or (isinstance(status, int) and status >= 400):
        if not copy.get("body"):
            copy["body"] = None
    else:
        copy.pop("body", None)
    return copy


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


def _title_from_a11y(items: list[dict[str, Any]]) -> str | None:
    for item in items:
        if item.get("role") == "RootWebArea":
            text = item.get("text")
            if text:
                return text
    return None


def build_observation(
    session_id: str,
    chrome_output: dict[str, Any],
    action_seq: int = 0,
    include_network: bool = True,
    include_console: bool = True,
    include_dom: bool = False,
    fresh: bool = True,
    max_debug_items: int = 20,
    dom_snapshot: list[dict[str, Any]] | None = None,
    dom_snapshot_error: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Build a full BrowserObservation from chrome-agent output."""
    session = STATE.sessions.setdefault(session_id, {})
    if not fresh:
        last = session.get("last_observation")
        if last is not None:
            return last

    viewport = session.get("viewport", {"width": 1365, "height": 768, "device_scale_factor": 1.0})
    url = chrome_output.get("url", session.get("url", ""))
    title = chrome_output.get("title", session.get("title", ""))

    screenshot = capture_screenshot(session_id, fresh=fresh)
    a11y_summary = parse_snapshot(chrome_output.get("snapshot", ""))

    if "title" not in chrome_output:
        title = _title_from_a11y(a11y_summary) or title

    session["url"] = url
    session["title"] = title

    network_summary = None
    console_summary = None
    pipe = get_pipe(session_id)
    listener = pipe._cdp_listener if pipe is not None else None
    raw_events: list[dict[str, Any]] = []
    if listener is not None:
        if not listener._connected.is_set():
            listener.wait_connected(timeout=2.0)
        if CDP_DRAIN_DELAY_SECONDS > 0:
            time.sleep(CDP_DRAIN_DELAY_SECONDS)
        raw_events = listener.drain_events()

    network_items: list[dict[str, Any]] = []
    console_items: list[dict[str, Any]] = []
    for event in raw_events:
        if event.get("type") == "console":
            console_items.append({
                "timestamp": now_iso(),
                "level": event.get("level", "info"),
                "text": event.get("text", ""),
                "source": event.get("source"),
                "line": event.get("line"),
            })
        else:
            network_items.append({
                "timestamp": now_iso(),
                "method": event.get("method", "GET"),
                "url_redacted": event.get("url", ""),
                "status": event.get("status"),
                "resource_type": event.get("resource_type", "other"),
                "error_text": event.get("error_text"),
                "body": event.get("body"),
            })

    if include_network:
        _merge_history(session, "network_history", network_items, action_seq)
        network_summary = summarize_network(
            [entry["item"] for entry in session.get("network_history", [])],
            limit=max_debug_items,
        )
    if include_console:
        _merge_history(session, "console_history", console_items, action_seq)
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
    if include_dom:
        if dom_snapshot is not None:
            observation["dom_snapshot"] = dom_snapshot
        if dom_snapshot_error is not None:
            observation["dom_snapshot_error"] = dom_snapshot_error
    session["last_observation"] = observation
    return observation


def _press_to_pipe_cmd(key: str, inspect_after: bool) -> list[dict[str, Any]]:
    """Map a press key to a chrome-agent pipe command list.

    Simple keys are sent through the native ``press`` command.  Combinations
    like ``ctrl+a`` are dispatched via a JavaScript ``KeyboardEvent`` so the
    sidecar does not depend on chrome-agent's key-combination syntax.
    """
    if "+" not in key:
        cmd: dict[str, Any] = {"cmd": "press", "key": key}
        if inspect_after:
            cmd["inspect"] = True
        return [cmd]

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
        return [{"cmd": "eval", "expression": "(() => { return 'Error: no key in combo'; })()"}]

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
    return [{"cmd": "eval", "expression": script}]


def _semantic_input_script(selector: str | None, value: str, fill: bool) -> str:
    """Build the single sidecar-owned semantic input primitive."""
    selector_json = json.dumps(selector)
    value_json = json.dumps(value)
    action_json = json.dumps("fill" if fill else "type_text")
    target_expr = (
        f"document.querySelector({selector_json})"
        if selector is not None
        else "document.activeElement"
    )
    return (
        "(() => { "
        "const selector = " + selector_json + "; "
        "const desired = " + value_json + "; "
        "const action = " + action_json + "; "
        "const el = " + target_expr + "; "
        "if (!el) return 'Error: no element found for semantic input'; "
        "const tag = (el.tagName || '').toLowerCase(); "
        "const common = { bubbles: true, cancelable: true, composed: true }; "
        "const dispatch = (type, init = {}) => { "
        "if (type === 'input' && typeof InputEvent === 'function') { "
        "el.dispatchEvent(new InputEvent(type, { ...common, data: desired, inputType: action === 'fill' ? 'insertReplacementText' : 'insertText', ...init })); "
        "} else { el.dispatchEvent(new Event(type, { ...common, ...init })); } "
        "}; "
        "const setterFrom = proto => proto && Object.getOwnPropertyDescriptor(proto, 'value') && Object.getOwnPropertyDescriptor(proto, 'value').set; "
        "const setNativeValue = next => { "
        "const ownSetter = Object.getOwnPropertyDescriptor(el, 'value') && Object.getOwnPropertyDescriptor(el, 'value').set; "
        "let nativeSetter = null; "
        "if (el instanceof HTMLInputElement) nativeSetter = setterFrom(HTMLInputElement.prototype); "
        "else if (el instanceof HTMLTextAreaElement) nativeSetter = setterFrom(HTMLTextAreaElement.prototype); "
        "else if (el instanceof HTMLSelectElement) nativeSetter = setterFrom(HTMLSelectElement.prototype); "
        "else nativeSetter = setterFrom(Object.getPrototypeOf(el)); "
        "const setter = nativeSetter && nativeSetter !== ownSetter ? nativeSetter : (ownSetter || nativeSetter); "
        "if (setter) setter.call(el, next); else el.value = next; "
        "}; "
        "if (!(tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable)) { "
        "return 'Error: element not fillable'; "
        "} "
        "if (typeof el.focus === 'function') el.focus({ preventScroll: true }); "
        "el.dispatchEvent(new FocusEvent('focus', common)); "
        "el.dispatchEvent(new FocusEvent('focusin', common)); "
        "if (el.isContentEditable) { "
        "dispatch('beforeinput'); "
        "el.textContent = desired; "
        "dispatch('input'); "
        "} else { "
        "dispatch('beforeinput'); "
        "setNativeValue(desired); "
        "dispatch('input'); "
        "} "
        "dispatch('change'); "
        "el.dispatchEvent(new KeyboardEvent('keyup', { ...common, key: 'End' })); "
        "const finalValue = el.isContentEditable ? el.textContent : el.value; "
        "if (finalValue !== desired) return 'Error: semantic input value mismatch; final length ' + String(finalValue ?? '').length + ', expected length ' + desired.length; "
        "return { ok: true, action, selector, tag, type: el.getAttribute('type'), value_length: String(finalValue ?? '').length, expected_length: desired.length, value_matches: true }; "
        "})()"
    )


def action_to_pipe_cmd(
    action: dict[str, Any], inspect_after: bool = True, timeout_ms: int | None = None
) -> list[dict[str, Any]]:
    """Translate a BrowserAction into a list of chrome-agent pipe commands.

    When `inspect_after` is False, mutating actions are issued without the
    chrome-agent `--inspect` flag. This is used by script execution so the
    sidecar can perform a single post-script inspect instead of one per step.

    Input actions (fill, type_text) use one sidecar-owned semantic value setter
    so React/Vue/Angular observe the same native-setter event sequence.
    """
    kind = action.get("kind")
    if kind == "click_xy":
        x = action.get("x", 0)
        y = action.get("y", 0)
        script = (
            "(() => { const el = document.elementFromPoint("
            + json.dumps(x) + ", " + json.dumps(y)
            + "); if (!el) return 'Error: no element at point'; "
            "const target = el.closest('button, a, [role=button], input[type=submit], [onclick]') || el; "
            "target.click(); return 'clicked ' + (target.tagName || 'element'); })()"
        )
        return [{"cmd": "eval", "expression": script, "inspect": inspect_after}]
    if kind == "click_selector":
        return [{"cmd": "click", "selector": action["selector"], "inspect": inspect_after}]
    if kind == "click_target_id":
        return [{"cmd": "click", "uid": action["target_id"], "inspect": inspect_after}]
    if kind == "fill":
        selector = action["selector"]
        value = action["value"]
        return [{"cmd": "eval", "expression": _semantic_input_script(selector, value, fill=True)}]
    if kind == "type_text":
        selector = action.get("selector")
        value = action.get("value", action.get("text", ""))
        return [{"cmd": "eval", "expression": _semantic_input_script(selector, value, fill=False)}]
    if kind == "press":
        return _press_to_pipe_cmd(action["key"], inspect_after=inspect_after)
    if kind == "scroll":
        dx = action.get("delta_x", 0)
        dy = action.get("delta_y", 0)
        return [{"cmd": "eval", "expression": f"window.scrollBy({dx},{dy}); true", "inspect": inspect_after}]
    if kind == "get_element_value":
        selector = action["selector"]
        return [{"cmd": "eval", "expression": f"(() => {{ const el = document.querySelector({json.dumps(selector)}); if (!el) return 'Error: element not found'; return el.value !== undefined ? el.value : el.textContent; }})()"}]
    if kind == "execute_javascript":
        expression = action["expression"]
        return [{"cmd": "eval", "expression": f"(() => {{ try {{ return ({expression}); }} catch (err) {{ return 'Error: ' + (err.message || err); }} }})()"}]
    if kind == "wait_for_selector":
        selector = action["selector"]
        timeout_s = max(1, (timeout_ms or action.get("timeout_ms", 10000)) // 1000)
        return [{"cmd": "wait", "what": "selector", "pattern": selector, "timeout": timeout_s}]
    if kind == "wait_for_text":
        text = action["text"]
        timeout_s = max(1, (timeout_ms or action.get("timeout_ms", 10000)) // 1000)
        return [{"cmd": "wait", "what": "text", "pattern": text, "timeout": timeout_s}]
    if kind == "wait":
        # chrome-agent pipe has no native "wait" command; the sidecar sleeps
        # in _handle_action before capturing the post-action observation.
        return [{"cmd": "eval", "expression": "true"}]
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


MUTATING_ACTION_KINDS = {
    "click_xy", "click_selector", "click_target_id", "fill", "type_text",
    "press", "scroll", "execute_javascript", "script",
}


def _is_mutating_action(action: dict[str, Any]) -> bool:
    """Return True if an action (or a script containing steps) mutates the page."""
    kind = action.get("kind")
    if kind == "script":
        return any(
            step.get("kind") in MUTATING_ACTION_KINDS
            for step in action.get("steps", [])
        )
    return kind in MUTATING_ACTION_KINDS


def _run_single_action_step(
    pipe: "ChromeAgentPipe", action: dict[str, Any], timeout: float = 60.0
) -> tuple[dict[str, Any], int]:
    """Execute one action step via the pipe and return (last_result, duration_ms)."""
    cmds = action_to_pipe_cmd(action, inspect_after=False, timeout_ms=action.get("timeout_ms"))
    started = time.time()
    result: dict[str, Any] = {}
    for cmd in cmds:
        result = pipe.send(cmd, timeout=timeout)
        if not result.get("ok"):
            break
    duration_ms = int((time.time() - started) * 1000)
    if action.get("kind") == "wait":
        timeout_ms = action.get("timeout_ms", 1000)
        time.sleep(timeout_ms / 1000)
    return result, duration_ms


class Handler(BaseHTTPRequestHandler):
    server_version = "oxide-browser-sidecar/0.3"

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
            include_dom = query.get("include_dom", ["false"])[0].lower() == "true"
            fresh = query.get("fresh", ["false"])[0].lower() == "true"
            max_debug_items = int(query.get("max_debug_items", ["20"])[0] or 20)
            self._handle_observe(session_id, include_network, include_console, include_dom, fresh, max_debug_items)
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

        if start_url == "about:blank":
            # chrome-agent may not accept about:blank; use a minimal valid URL.
            start_url = "https://www.google.com"

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

        try:
            pipe = ChromeAgentPipe(session_id, task_id)
        except Exception as exc:
            STATE.sessions.pop(session_id, None)
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": {
                    "code": "sidecar_not_ready",
                    "message": f"failed to start chrome-agent pipe: {exc}",
                    "retryable": True,
                    "hint": "ensure chrome-agent is installed and chromium is available",
                },
            })
            return

        STATE.pipes[session_id] = pipe
        result = pipe.send({"cmd": "goto", "url": start_url}, timeout=60)
        if not result.get("ok"):
            pipe.close(purge=True)
            STATE.pipes.pop(session_id, None)
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

    def _refresh_session_url_from_location(self, session_id: str, session: dict[str, Any]) -> str:
        """Refresh session URL from the browser's current window.location.href.

        This is a defensive check: CDP Page.frameNavigated updates the URL as
        events arrive, but for SPA hash navigation the stored value can still be
        stale by the time a new `goto` is issued. We fall back to the existing
        session URL if the eval fails.
        """
        pipe = get_pipe(session_id)
        result = pipe.send({"cmd": "eval", "expression": "window.location.href || document.URL || ''"}, timeout=10)
        if result.get("ok"):
            href = result.get("result", "")
            if isinstance(href, str) and href and href.startswith("http"):
                session["url"] = href
                return href
        return session.get("url", "")

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
        force_reload = bool(body.get("force_reload", False))
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

        pipe = get_pipe(session_id)
        action_seq = session.get("action_seq", 0)
        if force_reload:
            task_id = session.get("task_id", "browser-task")
            fresh_pipe, fresh_error = restart_pipe_with_fresh_browser(session_id, task_id, pipe)
            STATE.pipes.pop(session_id, None)
            if fresh_pipe is None:
                self.write_json(HTTPStatus.OK, {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": False,
                    "navigation": {
                        "url": url,
                        "final_url": session.get("url", ""),
                        "status": "blocked",
                        "http_status": None,
                        "redirect_count": 0,
                        "force_reload": force_reload,
                    },
                    "observation": None,
                    "error": fresh_error,
                })
                return
            pipe = fresh_pipe
            STATE.pipes[session_id] = pipe
            session["last_screenshot"] = None
            session["last_observation"] = None
            current_url = ""
        else:
            current_url = self._refresh_session_url_from_location(session_id, session)

        # SPA hash navigation: same origin and path, only the hash changed.
        if _is_same_origin_path_hash_navigation(current_url, url):
            parsed_target = urlparse(url)
            hash_value = parsed_target.fragment
            script = f"window.location.hash = {json.dumps('#' + hash_value)}; true"
            pipe.send({"cmd": "eval", "expression": script}, timeout=15)
            # Wait for the SPA to finish its async work. Network idle means XHRs
            # triggered by the hash change have completed; the selector fallback
            # ensures the DOM is actually rendered before we inspect.
            listener = pipe._cdp_listener if pipe is not None else None
            if listener is not None and listener._connected.is_set():
                listener.wait_for_network_idle(timeout=2.0)
            else:
                time.sleep(0.5)
            try:
                pipe.send({"cmd": "wait", "what": "selector", "pattern": "body", "timeout": 5}, timeout=10)
            except Exception:
                pass
            inspect_result = pipe.send({"cmd": "inspect"}, timeout=15)
            chrome_output = inspect_result if inspect_result.get("ok") else {"url": url, "title": session.get("title", "")}
            # Prefer the real location from the browser over the requested URL.
            final_url = self._refresh_session_url_from_location(session_id, session) or url
            session["url"] = final_url
            dom_snapshot, dom_snapshot_error = capture_dom_snapshot(pipe)
            observation = build_observation(
                session_id,
                chrome_output,
                action_seq=action_seq,
                include_dom=True,
                dom_snapshot=dom_snapshot,
                dom_snapshot_error=dom_snapshot_error,
                max_debug_items=20,
            )
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": True,
                "navigation": {
                    "url": url,
                    "final_url": final_url,
                    "status": "loaded",
                    "http_status": None,
                    "redirect_count": 0,
                    "force_reload": force_reload,
                },
                "observation": observation,
                "error": None,
            })
            return

        wait_until = str(body.get("wait_until", "load")).lower()
        result = pipe.send({"cmd": "goto", "url": url}, timeout=60)
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

        if wait_until == "networkidle":
            listener = pipe._cdp_listener if pipe is not None else None
            if listener is not None and listener._connected.is_set():
                listener.wait_for_network_idle(timeout=2.0)

        inspect_result = pipe.send({"cmd": "inspect"}, timeout=15)
        chrome_output = inspect_result if inspect_result.get("ok") else result
        # Update session URL/title before building the observation so the next
        # navigation decision sees the real page location.
        final_url = chrome_output.get("url") or result.get("url") or url
        session["url"] = final_url
        session["title"] = chrome_output.get("title") or result.get("title") or session.get("title", "")
        dom_snapshot, dom_snapshot_error = capture_dom_snapshot(pipe)
        observation = build_observation(
            session_id,
            chrome_output,
            action_seq=action_seq,
            include_dom=True,
            dom_snapshot=dom_snapshot,
            dom_snapshot_error=dom_snapshot_error,
            max_debug_items=20,
        )
        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": True,
            "navigation": {
                "url": url,
                "final_url": final_url,
                "status": "loaded",
                "http_status": None,
                "redirect_count": 0,
                "force_reload": force_reload,
            },
            "observation": observation,
            "error": None,
        })


    def _handle_observe(
        self,
        session_id: str,
        include_network: bool,
        include_console: bool,
        include_dom: bool,
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
        pipe = get_pipe(session_id)
        result = pipe.send({"cmd": "inspect"}, timeout=15)
        if not result.get("ok"):
            self.write_json(HTTPStatus.OK, {
                "request_id": request_id(),
                "session_id": session_id,
                "ok": False,
                "error": result.get("error"),
            })
            return

        dom_snapshot = None
        dom_snapshot_error = None
        if include_dom:
            dom_snapshot, dom_snapshot_error = capture_dom_snapshot(pipe)

        observation = build_observation(
            session_id,
            result,
            action_seq=action_seq,
            include_network=include_network,
            include_console=include_console,
            include_dom=include_dom,
            fresh=fresh,
            max_debug_items=max_debug_items,
            dom_snapshot=dom_snapshot,
            dom_snapshot_error=dom_snapshot_error,
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

        kind = action.get("kind", "unknown")
        pipe = get_pipe(session_id)

        result: dict[str, Any] = {}
        duration_ms = 0
        step_results: list[dict[str, Any]] = []
        last_error: Any = None

        if kind == "script":
            steps = action.get("steps", [])
            if not steps:
                self.write_json(HTTPStatus.OK, {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": False,
                    "action_result": {
                        "action_seq": action_seq,
                        "kind": "script",
                        "status": "failed",
                        "duration_ms": 0,
                        "technical_success": False,
                        "hint": "script has no steps",
                    },
                    "post_observation": None,
                    "error": {
                        "code": "invalid_action",
                        "message": "script has no steps",
                        "retryable": False,
                        "hint": "provide 1-10 executable steps",
                    },
                })
                return
            try:
                for step in steps:
                    action_to_pipe_cmd(step, inspect_after=False, timeout_ms=step.get("timeout_ms"))
            except ValueError as exc:
                self.write_json(HTTPStatus.OK, {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": False,
                    "action_result": {
                        "action_seq": action_seq,
                        "kind": "script",
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
                        "hint": "use a supported action kind inside the script",
                    },
                })
                return

            started = time.time()
            for step in steps:
                step_result, _ = _run_single_action_step(pipe, step, timeout=60)
                step_results.append(step_result)
                if not step_result.get("ok"):
                    last_error = step_result.get("error")
                    break
            duration_ms = int((time.time() - started) * 1000)
            result = step_results[-1] if step_results else {}
            success = all(r.get("ok") for r in step_results)
        else:
            try:
                cmds = action_to_pipe_cmd(action, inspect_after=True, timeout_ms=action.get("timeout_ms"))
            except ValueError as exc:
                self.write_json(HTTPStatus.OK, {
                    "request_id": request_id(),
                    "session_id": session_id,
                    "ok": False,
                    "action_result": {
                        "action_seq": action_seq,
                        "kind": kind,
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

            started = time.time()
            result: dict[str, Any] = {}
            for cmd in cmds:
                result = pipe.send(cmd, timeout=60)
                if not result.get("ok"):
                    break
            duration_ms = int((time.time() - started) * 1000)
            success = result.get("ok", False)
            if kind == "wait":
                timeout_ms = action.get("timeout_ms", 1000)
                time.sleep(timeout_ms / 1000)

        is_mutating = _is_mutating_action(action)

        post_observation = None
        if success and capture_after:
            chrome_output = result
            if is_mutating:
                # chrome-agent action results may only contain a message or a snapshot
                # without url/title. Fetch a fresh inspect to get the authoritative
                # post-action page state for verification.
                inspect_result = pipe.send({"cmd": "inspect"}, timeout=15)
                chrome_output = inspect_result if inspect_result.get("ok") else result
                # Give in-flight network requests a moment to complete so the
                # post-action observation reflects the real result of the action.
                # JS-driven actions may dispatch async XHR shortly after the
                # command returns; wait briefly so the listener sees them.
                listener = pipe._cdp_listener if pipe is not None else None
                if listener is not None and listener._connected.is_set():
                    time.sleep(0.2)
                    listener.wait_for_network_idle(timeout=2.0)
            dom_snapshot, dom_snapshot_error = capture_dom_snapshot(pipe)
            post_observation = build_observation(
                session_id,
                chrome_output,
                action_seq=action_seq,
                include_dom=True,
                dom_snapshot=dom_snapshot,
                dom_snapshot_error=dom_snapshot_error,
                max_debug_items=20,
            )

        result_value = None
        if success:
            if kind in ("fill", "type_text", "get_element_value", "execute_javascript"):
                result_value = _extract_eval_result(result)
            elif kind == "script" and step_results:
                steps = action.get("steps", [])
                if steps:
                    last_step = steps[-1]
                    last_step_kind = last_step.get("kind")
                    if last_step_kind in ("get_element_value", "execute_javascript"):
                        result_value = _extract_eval_result(step_results[-1])

        eval_error = (
            result_value is not None
            and isinstance(result_value, str)
            and result_value.startswith("Error:")
        )
        if success and eval_error:
            success = False

        if not success and last_error is None:
            raw_error = result.get("error")
            if isinstance(raw_error, dict):
                last_error = raw_error
            else:
                last_error = {
                    "code": "action_failed",
                    "message": str(raw_error) if raw_error else "action failed",
                    "retryable": False,
                    "hint": "",
                }

        self.write_json(HTTPStatus.OK, {
            "request_id": request_id(),
            "session_id": session_id,
            "ok": success,
            "action_result": {
                "action_seq": action_seq,
                "kind": kind,
                "status": "failed" if not success else "executed",
                "duration_ms": duration_ms,
                "technical_success": success,
                "hint": (
                    result_value
                    if (not success and eval_error)
                    else (last_error.get("hint") if isinstance(last_error, dict) else "")
                ),
                "result": None if not success else result_value,
            },
            "post_observation": post_observation,
            "error": last_error if not success else None,
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
        pipe = STATE.pipes.pop(session_id, None)
        result = {"ok": True}
        if pipe is not None:
            result = pipe.close(purge=purge)
        elif purge:
            # No pipe object but profile purge requested; use standalone CLI.
            result = run_chrome_agent(session_id, ["close", "--purge"], timeout=15)

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
    # Quick pipe smoke: create a pipe and navigate to a known URL.
    try:
        pipe = ChromeAgentPipe("self-test-pipe", "self-test")
        result = pipe.send({"cmd": "goto", "url": "https://example.com"}, timeout=30)
        if not result.get("ok"):
            print(f"chrome-agent-sidecar self-test: pipe goto failed: {result}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        marker_result = pipe.send({
            "cmd": "eval",
            "expression": "window.__oxide_fresh_navigation_marker = 'stale'; window.location.href",
        }, timeout=10)
        if not marker_result.get("ok"):
            print(f"chrome-agent-sidecar self-test: marker eval failed: {marker_result}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        fresh_pipe, fresh_error = restart_pipe_with_fresh_browser("self-test-pipe", "self-test", pipe)
        if fresh_pipe is None:
            print(f"chrome-agent-sidecar self-test: fresh restart failed: {fresh_error}", file=sys.stderr)
            run_chrome_agent("self-test-pipe", ["close", "--purge"], timeout=15)
            return 1
        pipe = fresh_pipe
        fresh_url = "https://example.com/#fresh-navigation"
        fresh_result = pipe.send({"cmd": "goto", "url": fresh_url}, timeout=30)
        if not fresh_result.get("ok"):
            print(f"chrome-agent-sidecar self-test: fresh goto failed: {fresh_result}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        fresh_marker = pipe.send({
            "cmd": "eval",
            "expression": "({ href: window.location.href, marker: window.__oxide_fresh_navigation_marker || null })",
        }, timeout=10)
        fresh_value = fresh_marker.get("result") if fresh_marker.get("ok") else None
        if not isinstance(fresh_value, dict) or fresh_value.get("href") != fresh_url or fresh_value.get("marker") is not None:
            print(f"chrome-agent-sidecar self-test: fresh navigation contract failed: {fresh_marker}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        dom_snapshot, dom_snapshot_error = capture_dom_snapshot(pipe)
        if dom_snapshot_error is not None or not isinstance(dom_snapshot, list):
            print(f"chrome-agent-sidecar self-test: DOM snapshot capture failed: {dom_snapshot_error}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        missing_snapshot, missing_error = capture_dom_snapshot(None)
        if missing_snapshot is not None or not isinstance(missing_error, dict) or missing_error.get("code") != "dom_snapshot_unavailable":
            print(f"chrome-agent-sidecar self-test: DOM snapshot failure contract failed: {missing_error}", file=sys.stderr)
            pipe.close(purge=True)
            return 1
        pipe.close(purge=True)
    except Exception as exc:
        print(f"chrome-agent-sidecar self-test: pipe smoke failed: {exc}", file=sys.stderr)
        return 1
    # Unit test: build_network_debug_payload respects include_bodies and only
    # exposes bodies for XHR/fetch/failed requests.
    _history = [
        {"action_seq": 1, "item": {"timestamp": "t1", "method": "POST", "url_redacted": "https://example.test/api", "status": 201, "resource_type": "xhr", "error_text": None, "body": "xhr-body"}},
        {"action_seq": 1, "item": {"timestamp": "t2", "method": "GET", "url_redacted": "https://example.test/img.png", "status": 200, "resource_type": "image", "error_text": None, "body": "image-body"}},
        {"action_seq": 1, "item": {"timestamp": "t3", "method": "GET", "url_redacted": "https://example.test/bad", "status": 500, "resource_type": "document", "error_text": None, "body": "error-body"}},
    ]
    _with = build_network_debug_payload(_history, 0, "all", "summary", True, 10)
    assert _with["items"][0].get("body") == "xhr-body", "missing XHR body"
    assert "body" not in _with["items"][1], "body leaked for non-XHR success"
    assert _with["items"][2].get("body") == "error-body", "missing failed body"
    _without = build_network_debug_payload(_history, 0, "all", "summary", False, 10)
    assert not any("body" in item for item in _without["items"]), "body present when disabled"

    fill_cmds = action_to_pipe_cmd({"kind": "fill", "selector": "#secret", "value": "hello"})
    type_cmds = action_to_pipe_cmd({"kind": "type_text", "selector": "#secret", "value": "hello"})
    assert len(fill_cmds) == 1 and fill_cmds[0]["cmd"] == "eval", "fill must use one semantic eval"
    assert len(type_cmds) == 1 and type_cmds[0]["cmd"] == "eval", "type_text must use one semantic eval"
    assert "HTMLInputElement.prototype" in fill_cmds[0]["expression"], "input native setter missing"
    assert "HTMLTextAreaElement.prototype" in fill_cmds[0]["expression"], "textarea native setter missing"
    assert "HTMLSelectElement.prototype" in fill_cmds[0]["expression"], "select native setter missing"
    assert "insertReplacementText" in fill_cmds[0]["expression"], "fill event intent missing"
    assert "insertText" in type_cmds[0]["expression"], "type_text event intent missing"
    assert _is_same_origin_path_hash_navigation(
        "https://example.test/app#old",
        "https://example.test/app#new",
    ), "same-origin hash navigation not detected"
    assert not _is_same_origin_path_hash_navigation(
        "https://example.test/app#old",
        "https://example.test/other#new",
    ), "different path misclassified as same-document hash navigation"

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
        STATE.reset()
    return 0


if __name__ == "__main__":
    sys.exit(main())
