from __future__ import annotations

import asyncio
import inspect
import json
import logging
import os
import shutil
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Literal
from uuid import uuid4

from fastapi import FastAPI, Header, HTTPException
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
ChatOpenAI = getattr(browser_use_module, "ChatOpenAI", None)

logger = logging.getLogger(__name__)


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
    max_profiles_per_scope: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_MAX_PROFILES_PER_SCOPE", 3
        )
    )
    profile_idle_ttl_secs: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS", 604800
        )
    )
    browser_ready_retries: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_BROWSER_READY_RETRIES", 2
        )
    )
    browser_ready_retry_delay_ms: int = field(
        default_factory=lambda: parse_int_env(
            "BROWSER_USE_BRIDGE_BROWSER_READY_RETRY_DELAY_MS", 750
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

MINIMAX_DEFAULT_API_BASE = "https://api.minimax.io/anthropic"
ZAI_DEFAULT_API_BASE = "https://api.z.ai/api/coding/paas/v4/chat/completions"
OPENAI_CHAT_COMPLETIONS_SUFFIX = "/chat/completions"
OXIDE_BROWSER_LLM_API_KEY_HEADER = "X-Oxide-Browser-Llm-Api-Key"


class BrowserLlmConfig(BaseModel):
    provider: str = Field(min_length=1)
    model: str | None = None
    api_base: str | None = None
    api_key_ref: str | None = None
    supports_vision: bool | None = None
    supports_tools: bool | None = None
    transport: str | None = None


class RunTaskRequest(BaseModel):
    task: str = Field(min_length=1)
    start_url: str | None = None
    session_id: str | None = None
    timeout_secs: int | None = Field(default=None, ge=1)
    reuse_profile: bool = False
    profile_id: str | None = Field(default=None, min_length=1)
    profile_scope: str | None = Field(default=None, min_length=1)
    browser_llm_config: BrowserLlmConfig | None = None


class RunTaskResponse(BaseModel):
    session_id: str
    status: Literal["running", "completed", "failed"]
    final_url: str | None = None
    summary: str | None = None
    artifacts: list[dict[str, Any]] = Field(default_factory=list)
    error: str | None = None
    llm_source: Literal["request_config", "legacy_env"] | None = None
    llm_provider: str | None = None
    llm_transport: str | None = None
    vision_mode: Literal["auto", "disabled"] | None = None
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False
    profile_reused: bool = False


class SessionResponse(BaseModel):
    session_id: str
    status: str
    current_url: str | None = None
    summary: str | None = None
    last_error: str | None = None
    llm_source: Literal["request_config", "legacy_env"] | None = None
    llm_provider: str | None = None
    llm_transport: str | None = None
    vision_mode: Literal["auto", "disabled"] | None = None
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False


class CloseSessionResponse(BaseModel):
    session_id: str
    closed: bool
    status: Literal["closed"]
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False


class ExtractContentRequest(BaseModel):
    format: Literal["text", "html"] = "text"
    max_chars: int | None = Field(default=12000, ge=1, le=100000)


class ExtractContentResponse(BaseModel):
    session_id: str
    status: Literal["completed"]
    current_url: str | None = None
    format: Literal["text", "html"]
    content: str
    truncated: bool
    total_chars: int


class ScreenshotRequest(BaseModel):
    full_page: bool = False


class ScreenshotResponse(BaseModel):
    session_id: str
    status: Literal["completed"]
    current_url: str | None = None
    artifact: dict[str, Any]


@dataclass(frozen=True)
class ResolvedBrowserLlmConfig:
    provider: str
    transport: str
    model: str | None
    api_base: str | None
    api_key: str | None
    supports_vision: bool | None
    supports_tools: bool | None
    source: Literal["request_config", "legacy_env"]


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
    llm_source: Literal["request_config", "legacy_env"] | None = None
    llm_provider: str | None = None
    llm_transport: str | None = None
    vision_mode: Literal["auto", "disabled"] | None = None
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False
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
            "llm_source": self.llm_source,
            "llm_provider": self.llm_provider,
            "llm_transport": self.llm_transport,
            "vision_mode": self.vision_mode,
            "profile_id": self.profile_id,
            "profile_scope": self.profile_scope,
            "profile_status": self.profile_status,
            "profile_attached": self.profile_attached,
        }


@dataclass
class ProfileRecord:
    profile_id: str
    profile_scope: str
    status: Literal["active", "idle", "stale", "deleted"] = "idle"
    current_session_id: str | None = None
    profile_dir: str = ""
    browser_data_dir: str = ""
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)
    last_used_at: str | None = None
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    def snapshot(self) -> dict[str, Any]:
        return {
            "profile_id": self.profile_id,
            "profile_scope": self.profile_scope,
            "status": self.status,
            "current_session_id": self.current_session_id,
            "profile_dir": self.profile_dir,
            "browser_data_dir": self.browser_data_dir,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "last_used_at": self.last_used_at,
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


def is_transient_browser_ready_error(error: Exception) -> bool:
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


def maybe_truncate(content: str, max_chars: int | None) -> tuple[str, bool, int]:
    total_chars = len(content)
    if max_chars is None or total_chars <= max_chars:
        return content, False, total_chars
    return content[:max_chars], True, total_chars


def normalize_name(value: str | None) -> str:
    if value is None:
        return ""
    return value.strip().lower().replace("-", "_")


def clean_optional(value: str | None) -> str | None:
    if value is None:
        return None
    cleaned = value.strip()
    return cleaned or None


def parse_timestamp(value: str | None) -> datetime | None:
    cleaned = clean_optional(value)
    if cleaned is None:
        return None

    try:
        parsed = datetime.fromisoformat(cleaned.replace("Z", "+00:00"))
    except ValueError:
        return None

    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def resolve_api_key_ref(secret_ref: str) -> str:
    secret_ref = secret_ref.strip()
    if not secret_ref:
        raise RuntimeError("browser_llm_config.api_key_ref must not be empty")

    if not secret_ref.startswith("env:"):
        raise RuntimeError(
            "browser_llm_config.api_key_ref currently supports only env:KEY"
        )

    env_name = secret_ref.removeprefix("env:").strip()
    if not env_name:
        raise RuntimeError("browser_llm_config.api_key_ref missing env name")

    value = os.getenv(env_name)
    if value is None or not value.strip():
        raise RuntimeError(
            f"browser_llm_config.api_key_ref points to missing env '{env_name}'"
        )
    return value.strip()


def normalize_openai_api_base(api_base: str | None) -> str | None:
    cleaned = clean_optional(api_base)
    if cleaned is None:
        return None
    trimmed = cleaned.rstrip("/")
    if trimmed.endswith(OPENAI_CHAT_COMPLETIONS_SUFFIX):
        trimmed = trimmed[: -len(OPENAI_CHAT_COMPLETIONS_SUFFIX)]
    return trimmed or None


def infer_transport(provider: str, api_base: str | None, transport: str | None) -> str:
    normalized_transport = normalize_name(transport)
    if normalized_transport:
        if normalized_transport in {
            "browser_use",
            "google",
            "anthropic",
            "anthropic_compatible",
            "openai_compatible",
        }:
            return normalized_transport
        raise RuntimeError(f"unsupported browser_llm_config.transport '{transport}'")

    if provider in {"browser_use"}:
        return "browser_use"
    if provider in {"google", "gemini"}:
        return "google"
    if provider in {"anthropic"}:
        return "anthropic"
    if provider in {"minimax"}:
        if api_base and "anthropic" not in api_base.lower():
            return "openai_compatible"
        return "anthropic_compatible"
    if provider in {
        "openai",
        "openai_compatible",
        "openrouter",
        "zai",
        "zhipuai",
        "glm",
    }:
        return "openai_compatible"
    raise RuntimeError(f"unsupported browser_llm_config.provider '{provider}'")


def resolve_requested_llm_config(
    config: BrowserLlmConfig, browser_llm_api_key: str | None
) -> ResolvedBrowserLlmConfig:
    provider = normalize_name(config.provider)
    if not provider:
        raise RuntimeError("browser_llm_config.provider is required")

    transport = infer_transport(provider, config.api_base, config.transport)
    model = clean_optional(config.model)
    if model is None:
        raise RuntimeError("browser_llm_config.model is required")

    api_base = clean_optional(config.api_base)
    if provider == "minimax" and api_base is None:
        api_base = MINIMAX_DEFAULT_API_BASE
    if provider in {"zai", "zhipuai", "glm"} and api_base is None:
        api_base = ZAI_DEFAULT_API_BASE
    if transport == "openai_compatible":
        api_base = normalize_openai_api_base(api_base)

    api_key = clean_optional(browser_llm_api_key)
    if api_key is None and clean_optional(config.api_key_ref) is not None:
        api_key = resolve_api_key_ref(config.api_key_ref)

    return ResolvedBrowserLlmConfig(
        provider=provider,
        transport=transport,
        model=model,
        api_base=api_base,
        api_key=api_key,
        supports_vision=config.supports_vision,
        supports_tools=config.supports_tools,
        source="request_config",
    )


def resolve_legacy_llm_config() -> ResolvedBrowserLlmConfig:
    provider = normalize_name(settings.llm_provider)
    if not provider:
        raise RuntimeError(
            "BROWSER_USE_BRIDGE_LLM_PROVIDER is required for browser task execution"
        )

    if provider not in {"browser_use", "google", "anthropic"}:
        raise RuntimeError(
            f"unsupported BROWSER_USE_BRIDGE_LLM_PROVIDER '{settings.llm_provider}'"
        )

    return ResolvedBrowserLlmConfig(
        provider=provider,
        transport=provider,
        model=clean_optional(settings.llm_model),
        api_base=None,
        api_key=None,
        supports_vision=None,
        supports_tools=None,
        source="legacy_env",
    )


def resolve_llm_config(
    request: RunTaskRequest, browser_llm_api_key: str | None
) -> ResolvedBrowserLlmConfig:
    if request.browser_llm_config is not None:
        return resolve_requested_llm_config(
            request.browser_llm_config, browser_llm_api_key
        )
    return resolve_legacy_llm_config()


def create_llm_from_config(config: ResolvedBrowserLlmConfig) -> Any:
    kwargs: dict[str, Any] = {}
    if config.model:
        kwargs["model"] = config.model

    if config.transport == "browser_use":
        if ChatBrowserUse is None:
            raise RuntimeError(
                "ChatBrowserUse is unavailable in installed browser_use package"
            )
        return ChatBrowserUse(**kwargs)

    if config.transport == "google":
        if ChatGoogle is None:
            raise RuntimeError(
                "ChatGoogle is unavailable in installed browser_use package"
            )
        return ChatGoogle(**kwargs)

    if config.transport in {"anthropic", "anthropic_compatible"}:
        if ChatAnthropic is None:
            raise RuntimeError(
                "ChatAnthropic is unavailable in installed browser_use package"
            )
        anthropic_kwargs = dict(kwargs)
        if config.api_key:
            anthropic_kwargs["api_key"] = config.api_key
        if config.api_base:
            anthropic_kwargs["base_url"] = config.api_base
        return ChatAnthropic(**anthropic_kwargs)

    if config.transport == "openai_compatible":
        if ChatOpenAI is None:
            raise RuntimeError(
                "ChatOpenAI is unavailable in installed browser_use package"
            )
        openai_kwargs = dict(kwargs)
        if config.api_key:
            openai_kwargs["api_key"] = config.api_key
        if config.api_base:
            openai_kwargs["base_url"] = config.api_base
        return ChatOpenAI(**openai_kwargs)

    raise RuntimeError(f"unsupported browser_llm transport '{config.transport}'")


def use_vision_mode(config: ResolvedBrowserLlmConfig) -> bool | Literal["auto"]:
    if config.supports_vision is False:
        return False
    return "auto"


def vision_mode_label(config: ResolvedBrowserLlmConfig) -> Literal["auto", "disabled"]:
    if config.supports_vision is False:
        return "disabled"
    return "auto"


async def resolve_browser_page(browser: Any) -> Any | None:
    for attr in ("page", "current_page"):
        candidate = getattr(browser, attr, None)
        if candidate is None:
            continue
        if callable(candidate):
            candidate = await maybe_await(candidate())
        if candidate is not None:
            return candidate
    return None


def invoke_with_supported_kwargs(method: Any, **kwargs: Any) -> Any:
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


class SessionManager:
    def __init__(
        self,
        data_dir: Path,
        max_concurrent_sessions: int,
        max_profiles_per_scope: int = 3,
        profile_idle_ttl_secs: int = 604800,
        browser_ready_retries: int = 2,
        browser_ready_retry_delay_ms: int = 750,
    ) -> None:
        self._data_dir = data_dir
        self._sessions_dir = data_dir / "sessions"
        self._artifacts_dir = data_dir / "artifacts"
        self._profiles_dir = data_dir / "profiles"
        self._sessions: dict[str, SessionRecord] = {}
        self._profiles: dict[str, ProfileRecord] = {}
        self._registry_lock = asyncio.Lock()
        self._semaphore = asyncio.Semaphore(max(1, max_concurrent_sessions))
        self._max_profiles_per_scope = max(1, max_profiles_per_scope)
        self._profile_idle_ttl_secs = max(0, profile_idle_ttl_secs)
        self._browser_ready_retries = max(0, browser_ready_retries)
        self._browser_ready_retry_delay_ms = max(0, browser_ready_retry_delay_ms)
        self._sessions_dir.mkdir(parents=True, exist_ok=True)
        self._artifacts_dir.mkdir(parents=True, exist_ok=True)
        self._profiles_dir.mkdir(parents=True, exist_ok=True)

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

    async def get_profile(self, profile_id: str) -> ProfileRecord:
        profile_id = profile_id.strip()
        if not profile_id:
            raise HTTPException(status_code=404, detail="unknown profile ''")

        await self._housekeep_profiles()

        async with self._registry_lock:
            profile = self._profiles.get(profile_id)
        if profile is not None:
            return profile

        metadata_path = self._profile_metadata_path(profile_id)
        if not metadata_path.exists():
            raise HTTPException(
                status_code=404, detail=f"unknown profile '{profile_id}'"
            )

        try:
            payload = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise HTTPException(
                status_code=500,
                detail=f"failed to load profile '{profile_id}': {error}",
            ) from error

        profile = ProfileRecord(
            profile_id=payload["profile_id"],
            profile_scope=payload.get("profile_scope", "bridge_local"),
            status=payload.get("status", "idle"),
            current_session_id=payload.get("current_session_id"),
            profile_dir=payload.get(
                "profile_dir", str(self._profile_root(profile_id).resolve())
            ),
            browser_data_dir=payload.get(
                "browser_data_dir", str(self._profile_browser_dir(profile_id).resolve())
            ),
            created_at=payload.get("created_at", utc_now()),
            updated_at=payload.get("updated_at", utc_now()),
            last_used_at=payload.get("last_used_at"),
        )
        async with self._registry_lock:
            self._profiles[profile.profile_id] = profile
        return profile

    async def create_session(self) -> SessionRecord:
        session = SessionRecord(session_id=f"browser-use-{uuid4().hex}")
        async with self._registry_lock:
            self._sessions[session.session_id] = session
        await self._persist(session)
        return session

    async def create_profile(
        self, profile_scope: str = "bridge_local"
    ) -> ProfileRecord:
        await self._housekeep_profiles()
        await self._enforce_profile_scope_quota(profile_scope)
        profile_id = f"browser-profile-{uuid4().hex}"
        profile_root = self._profile_root(profile_id)
        browser_data_dir = self._profile_browser_dir(profile_id)
        profile_root.mkdir(parents=True, exist_ok=True)
        browser_data_dir.mkdir(parents=True, exist_ok=True)

        profile = ProfileRecord(
            profile_id=profile_id,
            profile_scope=profile_scope,
            profile_dir=str(profile_root.resolve()),
            browser_data_dir=str(browser_data_dir.resolve()),
        )
        async with self._registry_lock:
            self._profiles[profile.profile_id] = profile
        await self._persist_profile(profile)
        return profile

    async def get_or_create_session(self, session_id: str | None) -> SessionRecord:
        if session_id is None:
            return await self.create_session()
        return await self.get_session(session_id)

    async def run_task(
        self, request: RunTaskRequest, browser_llm_api_key: str | None
    ) -> RunTaskResponse:
        await self.ensure_runtime_ready()
        session = await self.get_or_create_session(request.session_id)
        profile_reused = False

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
                    profile, profile_reused = await self._resolve_profile_for_run(
                        session, request
                    )
                    if profile is not None:
                        await self._attach_profile(session, profile)
                    else:
                        session.profile_attached = False

                    llm_config = resolve_llm_config(request, browser_llm_api_key)
                    session.llm_source = llm_config.source
                    session.llm_provider = llm_config.provider
                    session.llm_transport = llm_config.transport
                    session.vision_mode = vision_mode_label(llm_config)
                    result = await self._run_agent_with_browser_ready_retry(
                        session=session,
                        request=request,
                        profile=profile,
                        llm_config=llm_config,
                        timeout_secs=timeout_secs,
                    )
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
            llm_source=session.llm_source,
            llm_provider=session.llm_provider,
            llm_transport=session.llm_transport,
            vision_mode=session.vision_mode,
            profile_id=session.profile_id,
            profile_scope=session.profile_scope,
            profile_status=session.profile_status,
            profile_attached=session.profile_attached,
            profile_reused=profile_reused,
        )

    async def close_session(self, session_id: str) -> CloseSessionResponse:
        session = await self.get_session(session_id)
        await self._close_session_record(session)
        return CloseSessionResponse(
            session_id=session_id,
            closed=True,
            status="closed",
            profile_id=session.profile_id,
            profile_scope=session.profile_scope,
            profile_status=session.profile_status,
            profile_attached=session.profile_attached,
        )

    async def extract_content(
        self, session_id: str, request: ExtractContentRequest
    ) -> ExtractContentResponse:
        await self.ensure_runtime_ready()
        session = await self.get_session(session_id)

        async with session.lock:
            browser = self._require_active_browser(session)
            content = await self._extract_content(browser, request.format)
            content, truncated, total_chars = maybe_truncate(content, request.max_chars)
            current_url = await infer_url(browser, None)
            session.current_url = current_url
            session.updated_at = utc_now()
            await self._persist(session)

        return ExtractContentResponse(
            session_id=session.session_id,
            status="completed",
            current_url=current_url,
            format=request.format,
            content=content,
            truncated=truncated,
            total_chars=total_chars,
        )

    async def screenshot(
        self, session_id: str, request: ScreenshotRequest
    ) -> ScreenshotResponse:
        await self.ensure_runtime_ready()
        session = await self.get_session(session_id)

        async with session.lock:
            browser = self._require_active_browser(session)
            artifact = await self._take_screenshot(
                session.session_id, browser, request.full_page
            )
            session.artifacts.append(artifact)
            current_url = await infer_url(browser, None)
            session.current_url = current_url
            session.updated_at = utc_now()
            await self._persist(session)

        return ScreenshotResponse(
            session_id=session.session_id,
            status="completed",
            current_url=current_url,
            artifact=artifact,
        )

    async def shutdown(self) -> None:
        async with self._registry_lock:
            sessions = list(self._sessions.values())
        for session in sessions:
            await self._close_session_record(session)

    async def _close_session_record(self, session: SessionRecord) -> None:
        async with session.lock:
            await self._close_browser(session.browser)
            session.browser = None
            session.status = "closed"
            await self._detach_profile(session)
            session.updated_at = utc_now()
            await self._persist(session)

    async def _run_agent_with_browser_ready_retry(
        self,
        session: SessionRecord,
        request: RunTaskRequest,
        profile: ProfileRecord | None,
        llm_config: ResolvedBrowserLlmConfig,
        timeout_secs: int,
    ) -> Any:
        max_attempts = self._browser_ready_retries + 1
        for attempt in range(1, max_attempts + 1):
            try:
                if session.browser is None:
                    session.browser = self._create_browser(profile)
                agent = Agent(
                    task=build_agent_task(request),
                    llm=create_llm_from_config(llm_config),
                    browser=session.browser,
                    use_vision=use_vision_mode(llm_config),
                )
                return await asyncio.wait_for(agent.run(), timeout=timeout_secs)
            except Exception as error:
                if attempt >= max_attempts or not is_transient_browser_ready_error(
                    error
                ):
                    raise

                logger.warning(
                    "Retrying Browser Use after transient readiness failure",
                    extra={
                        "session_id": session.session_id,
                        "attempt": attempt,
                        "max_attempts": max_attempts,
                        "error": str(error),
                    },
                )
                await self._reset_browser_for_retry(session)
                if self._browser_ready_retry_delay_ms > 0:
                    await asyncio.sleep(self._browser_ready_retry_delay_ms / 1000)

        raise RuntimeError("browser readiness retry loop exhausted")

    async def _reset_browser_for_retry(self, session: SessionRecord) -> None:
        await self._close_browser(session.browser)
        session.browser = None

    async def _housekeep_profiles(self) -> None:
        await self._reconcile_orphaned_profiles()
        await self._prune_expired_profiles()

    async def _reconcile_orphaned_profiles(self) -> None:
        async with self._registry_lock:
            live_sessions = {
                session_id: session.snapshot()
                for session_id, session in self._sessions.items()
            }

        for metadata_path in self._profiles_dir.glob("*/metadata.json"):
            payload = self._load_profile_payload(metadata_path)
            if payload is None or payload.get("status") != "active":
                continue

            current_session_id = clean_optional(payload.get("current_session_id"))
            profile_id = payload.get("profile_id")
            if not isinstance(profile_id, str) or not profile_id.strip():
                continue

            if (
                current_session_id is not None
                and self._session_snapshot_matches_profile(
                    live_sessions.get(current_session_id), profile_id
                )
            ):
                continue

            payload["status"] = "stale"
            payload["current_session_id"] = None
            payload["updated_at"] = utc_now()
            self._write_profile_payload(metadata_path, payload)
            await self._sync_cached_profile_payload(payload)

    async def _prune_expired_profiles(self) -> None:
        if self._profile_idle_ttl_secs <= 0:
            return

        cutoff = datetime.now(timezone.utc) - timedelta(
            seconds=self._profile_idle_ttl_secs
        )
        for metadata_path in self._profiles_dir.glob("*/metadata.json"):
            payload = self._load_profile_payload(metadata_path)
            if payload is None:
                continue

            status = payload.get("status")
            if status == "active":
                continue

            if not self._profile_payload_is_expired(payload, cutoff):
                continue

            profile_id = payload.get("profile_id")
            if not isinstance(profile_id, str) or not profile_id.strip():
                continue

            try:
                shutil.rmtree(metadata_path.parent)
            except OSError:
                continue

            async with self._registry_lock:
                self._profiles.pop(profile_id, None)

    def _load_profile_payload(self, metadata_path: Path) -> dict[str, Any] | None:
        try:
            payload = json.loads(metadata_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None

        if isinstance(payload, dict):
            return payload
        return None

    def _write_profile_payload(
        self, metadata_path: Path, payload: dict[str, Any]
    ) -> None:
        metadata_path.parent.mkdir(parents=True, exist_ok=True)
        metadata_path.write_text(
            json.dumps(payload, ensure_ascii=True, indent=2),
            encoding="utf-8",
        )

    def _session_snapshot_matches_profile(
        self, snapshot: dict[str, Any] | None, profile_id: str
    ) -> bool:
        if snapshot is None:
            return False
        if snapshot.get("profile_id") != profile_id:
            return False
        if not snapshot.get("profile_attached"):
            return False
        return snapshot.get("status") != "closed"

    def _profile_payload_is_expired(
        self, payload: dict[str, Any], cutoff: datetime
    ) -> bool:
        if payload.get("status") == "deleted":
            return True

        for key in ("last_used_at", "updated_at", "created_at"):
            parsed = parse_timestamp(payload.get(key))
            if parsed is not None:
                return parsed <= cutoff
        return False

    async def _sync_cached_profile_payload(self, payload: dict[str, Any]) -> None:
        profile_id = payload.get("profile_id")
        if not isinstance(profile_id, str) or not profile_id.strip():
            return

        async with self._registry_lock:
            profile = self._profiles.get(profile_id)
        if profile is None:
            return

        profile.profile_scope = payload.get("profile_scope", profile.profile_scope)
        profile.status = payload.get("status", profile.status)
        profile.current_session_id = payload.get("current_session_id")
        profile.profile_dir = payload.get("profile_dir", profile.profile_dir)
        profile.browser_data_dir = payload.get(
            "browser_data_dir", profile.browser_data_dir
        )
        profile.created_at = payload.get("created_at", profile.created_at)
        profile.updated_at = payload.get("updated_at", profile.updated_at)
        profile.last_used_at = payload.get("last_used_at")

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

    async def _resolve_profile_for_run(
        self, session: SessionRecord, request: RunTaskRequest
    ) -> tuple[ProfileRecord | None, bool]:
        requested_profile_id = clean_optional(request.profile_id)
        requested_profile_scope = clean_optional(request.profile_scope)

        if session.browser is not None:
            if requested_profile_id is not None:
                if session.profile_id is None:
                    raise HTTPException(
                        status_code=409,
                        detail=(
                            "cannot attach persistent profile to an already active "
                            "ephemeral session; close the session or start a new one"
                        ),
                    )
                if session.profile_id != requested_profile_id:
                    raise HTTPException(
                        status_code=409,
                        detail=(
                            f"session '{session.session_id}' is already attached to "
                            f"profile '{session.profile_id}'"
                        ),
                    )
            elif request.reuse_profile and session.profile_id is None:
                raise HTTPException(
                    status_code=409,
                    detail=(
                        "cannot enable persistent profile reuse for an already active "
                        "ephemeral session; close the session or start a new one"
                    ),
                )

            if session.profile_id is not None:
                if (
                    requested_profile_scope is not None
                    and session.profile_scope != requested_profile_scope
                ):
                    raise HTTPException(
                        status_code=409,
                        detail=(
                            f"session '{session.session_id}' is already attached to "
                            f"profile scope '{session.profile_scope}'"
                        ),
                    )
                return await self.get_profile(session.profile_id), True
            return None, False

        if requested_profile_id is not None:
            profile = await self.get_profile(requested_profile_id)
            if (
                requested_profile_scope is not None
                and profile.profile_scope != requested_profile_scope
            ):
                raise HTTPException(
                    status_code=409,
                    detail=(
                        f"profile '{profile.profile_id}' belongs to scope "
                        f"'{profile.profile_scope}', not '{requested_profile_scope}'"
                    ),
                )
            return profile, True
        if request.reuse_profile:
            return await self.create_profile(
                requested_profile_scope or "bridge_local"
            ), False
        return None, False

    async def _enforce_profile_scope_quota(self, profile_scope: str) -> None:
        retained = await self._count_profiles_for_scope(profile_scope)
        if retained >= self._max_profiles_per_scope:
            raise HTTPException(
                status_code=409,
                detail=(
                    f"profile scope '{profile_scope}' already has "
                    f"{retained} retained profiles; max is {self._max_profiles_per_scope}"
                ),
            )

    async def _count_profiles_for_scope(self, profile_scope: str) -> int:
        count = 0
        for metadata_path in self._profiles_dir.glob("*/metadata.json"):
            try:
                payload = json.loads(metadata_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if payload.get("profile_scope") != profile_scope:
                continue
            if payload.get("status") == "deleted":
                continue
            count += 1
        return count

    async def _attach_profile(
        self, session: SessionRecord, profile: ProfileRecord
    ) -> None:
        async with profile.lock:
            if (
                profile.current_session_id is not None
                and profile.current_session_id != session.session_id
            ):
                raise HTTPException(
                    status_code=409,
                    detail=(
                        f"profile '{profile.profile_id}' is already attached to "
                        f"session '{profile.current_session_id}'"
                    ),
                )
            profile.current_session_id = session.session_id
            profile.status = "active"
            profile.last_used_at = utc_now()
            profile.updated_at = utc_now()
            await self._persist_profile(profile)

        session.profile_id = profile.profile_id
        session.profile_scope = profile.profile_scope
        session.profile_status = profile.status
        session.profile_attached = True

    async def _detach_profile(self, session: SessionRecord) -> None:
        if session.profile_id is None:
            session.profile_attached = False
            return

        try:
            profile = await self.get_profile(session.profile_id)
        except HTTPException:
            session.profile_attached = False
            session.profile_status = "idle"
            return

        async with profile.lock:
            if profile.current_session_id == session.session_id:
                profile.current_session_id = None
            profile.status = "idle"
            profile.updated_at = utc_now()
            await self._persist_profile(profile)

        session.profile_scope = profile.profile_scope
        session.profile_status = profile.status
        session.profile_attached = False

    def _create_browser(self, profile: ProfileRecord | None) -> Any:
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

    def _profile_root(self, profile_id: str) -> Path:
        return self._profiles_dir / profile_id

    def _profile_browser_dir(self, profile_id: str) -> Path:
        return self._profile_root(profile_id) / "browser"

    def _profile_metadata_path(self, profile_id: str) -> Path:
        return self._profile_root(profile_id) / "metadata.json"

    def _require_active_browser(self, session: SessionRecord) -> Any:
        if session.browser is None:
            raise HTTPException(
                status_code=409,
                detail=(
                    f"session '{session.session_id}' has no active browser; run "
                    "browser_use_run_task first"
                ),
            )
        return session.browser

    async def _extract_content(self, browser: Any, content_format: str) -> str:
        page = await resolve_browser_page(browser)

        if content_format == "html":
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

            state = await self._browser_state(browser)
            for key in ("html", "page_html", "content"):
                value = state.get(key)
                if isinstance(value, str) and value.strip():
                    return value

            if page is not None:
                evaluate = getattr(page, "evaluate", None)
                if callable(evaluate):
                    result = await maybe_await(
                        evaluate("document.documentElement.outerHTML")
                    )
                    if isinstance(result, str) and result.strip():
                        return result

            raise HTTPException(
                status_code=500,
                detail="browser_use bridge could not extract HTML from active session",
            )

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

        state = await self._browser_state(browser)
        for key in ("text", "page_text", "content"):
            value = state.get(key)
            if isinstance(value, str) and value.strip():
                return value

        raise HTTPException(
            status_code=500,
            detail="browser_use bridge could not extract page content from active session",
        )

    async def _browser_state(self, browser: Any) -> dict[str, Any]:
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

    async def _take_screenshot(
        self, session_id: str, browser: Any, full_page: bool
    ) -> dict[str, Any]:
        session_artifacts_dir = self._artifacts_dir / session_id
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

        artifact = {
            "kind": "screenshot",
            "path": str(path),
            "full_page": full_page,
            "size_bytes": path.stat().st_size,
            "created_at": utc_now(),
        }
        return artifact

    async def _persist(self, session: SessionRecord) -> None:
        session_file = self._sessions_dir / f"{session.session_id}.json"
        session_file.write_text(
            json.dumps(session.snapshot(), ensure_ascii=True, indent=2),
            encoding="utf-8",
        )

    async def _persist_profile(self, profile: ProfileRecord) -> None:
        metadata_path = self._profile_metadata_path(profile.profile_id)
        metadata_path.parent.mkdir(parents=True, exist_ok=True)
        metadata_path.write_text(
            json.dumps(profile.snapshot(), ensure_ascii=True, indent=2),
            encoding="utf-8",
        )


manager = SessionManager(
    data_dir=settings.data_dir,
    max_concurrent_sessions=settings.max_concurrent_sessions,
    max_profiles_per_scope=settings.max_profiles_per_scope,
    profile_idle_ttl_secs=settings.profile_idle_ttl_secs,
    browser_ready_retries=settings.browser_ready_retries,
    browser_ready_retry_delay_ms=settings.browser_ready_retry_delay_ms,
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
        "preferred_browser_llm_source": "request_browser_llm_config",
        "request_browser_llm_config_supported": True,
        "request_browser_llm_api_key_header_supported": True,
        "browser_llm_api_key_header": OXIDE_BROWSER_LLM_API_KEY_HEADER,
        "legacy_env_fallback_configured": clean_optional(settings.llm_provider)
        is not None,
        "legacy_env_llm_provider": clean_optional(settings.llm_provider),
        "legacy_env_llm_model": clean_optional(settings.llm_model),
        "supported_legacy_env_providers": ["browser_use", "google", "anthropic"],
        "supported_inherited_route_providers": [
            "gemini",
            "minimax",
            "zai",
            "openrouter",
        ],
        "supported_browser_llm_providers": [
            "browser_use",
            "google",
            "anthropic",
            "minimax",
            "zai",
            "openrouter",
            "openai_compatible",
        ],
        "profile_reuse_supported": True,
        "profile_scope_mode": "runtime_injected_preferred",
        "max_profiles_per_scope": settings.max_profiles_per_scope,
        "profile_idle_ttl_secs": settings.profile_idle_ttl_secs,
        "browser_ready_retries": settings.browser_ready_retries,
        "browser_ready_retry_delay_ms": settings.browser_ready_retry_delay_ms,
        "browser_ready_retry_supported": True,
        "orphan_profile_recovery_supported": True,
    }
    status_code = 200 if BROWSER_USE_IMPORT_ERROR is None else 503
    return JSONResponse(content=payload, status_code=status_code)


@app.post("/sessions/run", response_model=RunTaskResponse)
async def run_session(
    request: RunTaskRequest,
    browser_llm_api_key: str | None = Header(
        default=None, alias=OXIDE_BROWSER_LLM_API_KEY_HEADER
    ),
) -> RunTaskResponse:
    return await manager.run_task(request, browser_llm_api_key)


@app.get("/sessions/{session_id}", response_model=SessionResponse)
async def get_session(session_id: str) -> SessionResponse:
    session = await manager.get_session(session_id)
    return SessionResponse(
        session_id=session.session_id,
        status=session.status,
        current_url=session.current_url,
        summary=session.summary,
        last_error=session.last_error,
        llm_source=session.llm_source,
        llm_provider=session.llm_provider,
        llm_transport=session.llm_transport,
        vision_mode=session.vision_mode,
        profile_id=session.profile_id,
        profile_scope=session.profile_scope,
        profile_status=session.profile_status,
        profile_attached=session.profile_attached,
    )


@app.delete("/sessions/{session_id}", response_model=CloseSessionResponse)
async def delete_session(session_id: str) -> CloseSessionResponse:
    return await manager.close_session(session_id)


@app.post(
    "/sessions/{session_id}/extract_content", response_model=ExtractContentResponse
)
async def extract_content(
    session_id: str, request: ExtractContentRequest
) -> ExtractContentResponse:
    return await manager.extract_content(session_id, request)


@app.post("/sessions/{session_id}/screenshot", response_model=ScreenshotResponse)
async def screenshot(session_id: str, request: ScreenshotRequest) -> ScreenshotResponse:
    return await manager.screenshot(session_id, request)
