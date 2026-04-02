"""Internal models for session and profile management."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Any, Literal

from app.utils.time import utc_now

ExecutionMode = Literal["autonomous", "navigation_only"]


@dataclass(frozen=True)
class ResolvedBrowserLlmConfig:
    """Resolved LLM configuration ready for use."""

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
    """Internal record for an active browser session."""

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
    execution_mode: ExecutionMode | None = None
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False
    browser_runtime_alive: bool | None = None
    browser_runtime_last_check_at: str | None = None
    browser_runtime_dead_reason: str | None = None
    browser_keep_alive_requested: bool | None = None
    browser_keep_alive_effective: bool | None = None
    browser_reconnect_attempted: bool | None = None
    browser_reconnect_succeeded: bool | None = None
    browser_reconnect_error: str | None = None
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)

    def snapshot(self) -> dict[str, Any]:
        """Return a JSON-serializable snapshot of the session."""
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
            "execution_mode": self.execution_mode,
            "profile_id": self.profile_id,
            "profile_scope": self.profile_scope,
            "profile_status": self.profile_status,
            "profile_attached": self.profile_attached,
            "browser_runtime_alive": self.browser_runtime_alive,
            "browser_runtime_last_check_at": self.browser_runtime_last_check_at,
            "browser_runtime_dead_reason": self.browser_runtime_dead_reason,
            "browser_keep_alive_requested": self.browser_keep_alive_requested,
            "browser_keep_alive_effective": self.browser_keep_alive_effective,
            "browser_reconnect_attempted": self.browser_reconnect_attempted,
            "browser_reconnect_succeeded": self.browser_reconnect_succeeded,
            "browser_reconnect_error": self.browser_reconnect_error,
        }


@dataclass
class ProfileRecord:
    """Internal record for a persistent browser profile."""

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
        """Return a JSON-serializable snapshot of the profile."""
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
