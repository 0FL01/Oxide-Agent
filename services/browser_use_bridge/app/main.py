from __future__ import annotations

import asyncio
import inspect
import json
import os
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal
from uuid import uuid4

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

try:
    import browser_use as browser_use_module
except ImportError as error:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None
    BROWSER_USE_IMPORT_ERROR = str(error)
else:
    BROWSER_USE_IMPORT_ERROR = None

Agent = getattr(browser_use_module, "Agent", None)
Browser = getattr(browser_use_module, "Browser", None)
ChatAnthropic = getattr(browser_use_module, "ChatAnthropic", None)
ChatBrowserUse = getattr(browser_use_module, "ChatBrowserUse", None)
ChatGoogle = getattr(browser_use_module, "ChatGoogle", None)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def parse_int_env(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return default
    try:
        return int(raw)
    except ValueError:
        return default


@dataclass(frozen=True)
class Settings:
    host: str = field(
        default_factory=lambda: os.getenv("BROWSER_USE_BRIDGE_HOST", "0.0.0.0")
    )
    port: int = field(
        default_factory=lambda: parse_int_env("BROWSER_USE_BRIDGE_PORT", 8000)
    )
    data_dir: Path = field(
        default_factory=lambda: Path(
            os.getenv("BROWSER_USE_BRIDGE_DATA_DIR", "/tmp/browser-use-bridge")
        )
    )
    default_timeout_secs: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_DEFAULT_TIMEOUT_SECS", 120
        )
    )
    max_timeout_secs: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_MAX_TIMEOUT_SECS", 300
        )
    )
    max_concurrent_sessions: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_MAX_CONCURRENT_SESSIONS", 2
        )
    )
    llm_provider: str = field(
        default_factory=lambda: os.getenv("BROWSER_USE_BRIDGE_LLM_PROVIDER", "").strip()
    )
    llm_model: str | None = field(
        default_factory=lambda: os.getenv("BROWSER_USE_BRIDGE_LLM_MODEL")
    )


settings = Settings()
os.environ.setdefault("BROWSER_USE_HOME", str(settings.data_dir))


class RunTaskRequest(BaseModel):
    task: str = Field(min_length=1)
    start_url: str | None = None
    session_id: str | None = None
    timeout_secs: int | None = Field(default=None, ge=1)


class RunTaskResponse(BaseModel):
    session_id: str
    status: Literal["running", "completed", "failed"]
    final_url: str | None = None
    summary: str | None = None
    artifacts: list[dict[str, Any]] = Field(default_factory=list)
    error: str | None = None


class SessionResponse(BaseModel):
    session_id: str
    status: str
    current_url: str | None = None
    summary: str | None = None
    last_error: str | None = None


class CloseSessionResponse(BaseModel):
    session_id: str
    closed: bool
    status: Literal["closed"]


@dataclass
class SessionRecord:
    session_id: str
    status: str = "idle"
    browser: Any | None = None
    summary: str | None = None
    current_url: str | None = None
    last_error: str | None = None
    last_task: str | None = None
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)
    artifacts: list[dict[str, Any]] = field(default_factory=list)
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    def snapshot(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "status": self.status,
            "summary": self.summary,
            "current_url": self.current_url,
            "last_error": self.last_error,
            "last_task": self.last_task,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "artifacts": self.artifacts,
        }


async def maybe_await(value: Any) -> Any:
    if inspect.isawaitable(value):
        return await value
    return value


def json_safe(value: Any) -> Any:
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
    if value is None:
        return "Task completed."
    if isinstance(value, str):
        text = value.strip()
        return text or "Task completed."
    for attr in ("final_result", "result", "summary", "message"):
        candidate = getattr(value, attr, None)
        if isinstance(candidate, str) and candidate.strip():
            return candidate.strip()
    return json.dumps(json_safe(value), ensure_ascii=True)


def extract_artifacts(value: Any) -> list[dict[str, Any]]:
    for attr in ("artifacts", "files", "screenshots"):
        candidate = getattr(value, attr, None)
        if candidate is None:
            continue
        safe = json_safe(candidate)
        if isinstance(safe, list):
            return [
                item if isinstance(item, dict) else {"value": item} for item in safe
            ]
    return []


async def infer_url(browser: Any, result: Any) -> str | None:
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


def build_agent_task(request: RunTaskRequest) -> str:
    task_parts = [request.task.strip()]
    if request.start_url:
        task_parts.append(f"Start from this URL: {request.start_url.strip()}")
    return "\n\n".join(task_parts)


def create_llm() -> Any:
    provider = settings.llm_provider.lower()
    if not provider:
        raise RuntimeError(
            "BROWSER_USE_BRIDGE_LLM_PROVIDER is required for browser task execution"
        )

    kwargs: dict[str, Any] = {}
    if settings.llm_model:
        kwargs["model"] = settings.llm_model

    if provider == "browser_use":
        if ChatBrowserUse is None:
            raise RuntimeError(
                "ChatBrowserUse is unavailable in installed browser_use package"
            )
        return ChatBrowserUse(**kwargs)

    if provider == "google":
        if ChatGoogle is None:
            raise RuntimeError(
                "ChatGoogle is unavailable in installed browser_use package"
            )
        return ChatGoogle(**kwargs)

    if provider == "anthropic":
        if ChatAnthropic is None:
            raise RuntimeError(
                "ChatAnthropic is unavailable in installed browser_use package"
            )
        return ChatAnthropic(**kwargs)

    raise RuntimeError(
        f"unsupported BROWSER_USE_BRIDGE_LLM_PROVIDER '{settings.llm_provider}'"
    )


class SessionManager:
    def __init__(self, data_dir: Path, max_concurrent_sessions: int) -> None:
        self._data_dir = data_dir
        self._sessions_dir = data_dir / "sessions"
        self._sessions: dict[str, SessionRecord] = {}
        self._registry_lock = asyncio.Lock()
        self._semaphore = asyncio.Semaphore(max(1, max_concurrent_sessions))
        self._sessions_dir.mkdir(parents=True, exist_ok=True)

    async def ensure_runtime_ready(self) -> None:
        if Agent is None or Browser is None:
            raise HTTPException(
                status_code=503,
                detail={
                    "error": "browser_use_unavailable",
                    "message": BROWSER_USE_IMPORT_ERROR or "browser_use import failed",
                },
            )

    async def get_session(self, session_id: str) -> SessionRecord:
        async with self._registry_lock:
            session = self._sessions.get(session_id)
        if session is None:
            raise HTTPException(
                status_code=404, detail=f"unknown session '{session_id}'"
            )
        return session

    async def create_session(self) -> SessionRecord:
        session = SessionRecord(session_id=f"browser-use-{uuid4().hex}")
        async with self._registry_lock:
            self._sessions[session.session_id] = session
        await self._persist(session)
        return session

    async def get_or_create_session(self, session_id: str | None) -> SessionRecord:
        if session_id is None:
            return await self.create_session()
        return await self.get_session(session_id)

    async def run_task(self, request: RunTaskRequest) -> RunTaskResponse:
        await self.ensure_runtime_ready()
        session = await self.get_or_create_session(request.session_id)

        async with session.lock:
            if session.status == "running":
                raise HTTPException(
                    status_code=409,
                    detail=f"session '{session.session_id}' is already running",
                )

            session.status = "running"
            session.last_error = None
            session.last_task = request.task.strip()
            session.updated_at = utc_now()
            await self._persist(session)

            timeout_secs = min(
                request.timeout_secs or settings.default_timeout_secs,
                settings.max_timeout_secs,
            )

            async with self._semaphore:
                try:
                    if session.browser is None:
                        session.browser = Browser()

                    agent = Agent(
                        task=build_agent_task(request),
                        llm=create_llm(),
                        browser=session.browser,
                    )
                    result = await asyncio.wait_for(agent.run(), timeout=timeout_secs)
                    session.summary = stringify_result(result)
                    session.artifacts = extract_artifacts(result)
                    session.current_url = await infer_url(session.browser, result)
                    session.status = "completed"
                except Exception as error:
                    session.status = "failed"
                    session.last_error = str(error)
                finally:
                    session.updated_at = utc_now()
                    await self._persist(session)

        return RunTaskResponse(
            session_id=session.session_id,
            status=session.status
            if session.status in {"running", "completed", "failed"}
            else "failed",
            final_url=session.current_url,
            summary=session.summary,
            artifacts=session.artifacts,
            error=session.last_error,
        )

    async def close_session(self, session_id: str) -> CloseSessionResponse:
        session = await self.get_session(session_id)
        async with session.lock:
            await self._close_browser(session.browser)
            session.browser = None
            session.status = "closed"
            session.updated_at = utc_now()
            await self._persist(session)
        return CloseSessionResponse(session_id=session_id, closed=True, status="closed")

    async def shutdown(self) -> None:
        async with self._registry_lock:
            sessions = list(self._sessions.values())
        await asyncio.gather(
            *(self._close_browser(session.browser) for session in sessions)
        )

    async def _close_browser(self, browser: Any) -> None:
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

    async def _persist(self, session: SessionRecord) -> None:
        session_file = self._sessions_dir / f"{session.session_id}.json"
        session_file.write_text(
            json.dumps(session.snapshot(), ensure_ascii=True, indent=2),
            encoding="utf-8",
        )


manager = SessionManager(
    data_dir=settings.data_dir,
    max_concurrent_sessions=settings.max_concurrent_sessions,
)
app = FastAPI(title="browser_use_bridge", version="0.1.0")


@app.on_event("shutdown")
async def on_shutdown() -> None:
    await manager.shutdown()


@app.get("/health")
async def health() -> JSONResponse:
    payload = {
        "status": "ok" if BROWSER_USE_IMPORT_ERROR is None else "unavailable",
        "browser_use_available": BROWSER_USE_IMPORT_ERROR is None,
        "import_error": BROWSER_USE_IMPORT_ERROR,
        "data_dir": str(settings.data_dir),
        "max_concurrent_sessions": settings.max_concurrent_sessions,
    }
    status_code = 200 if BROWSER_USE_IMPORT_ERROR is None else 503
    return JSONResponse(content=payload, status_code=status_code)


@app.post("/sessions/run", response_model=RunTaskResponse)
async def run_session(request: RunTaskRequest) -> RunTaskResponse:
    return await manager.run_task(request)


@app.get("/sessions/{session_id}", response_model=SessionResponse)
async def get_session(session_id: str) -> SessionResponse:
    session = await manager.get_session(session_id)
    return SessionResponse(
        session_id=session.session_id,
        status=session.status,
        current_url=session.current_url,
        summary=session.summary,
        last_error=session.last_error,
    )


@app.delete("/sessions/{session_id}", response_model=CloseSessionResponse)
async def delete_session(session_id: str) -> CloseSessionResponse:
    return await manager.close_session(session_id)
