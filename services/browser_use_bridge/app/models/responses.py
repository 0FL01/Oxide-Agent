"""Response models for API endpoints."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field


class RunTaskResponse(BaseModel):
    """Response from running a browser task."""

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
    """Response with session status."""

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
    """Response from closing a session."""

    session_id: str
    closed: bool
    status: Literal["closed"]
    profile_id: str | None = None
    profile_scope: str | None = None
    profile_status: str | None = None
    profile_attached: bool = False


class ExtractContentResponse(BaseModel):
    """Response from extracting content."""

    session_id: str
    status: Literal["completed"]
    current_url: str | None = None
    format: Literal["text", "html"]
    content: str
    truncated: bool
    total_chars: int


class ScreenshotResponse(BaseModel):
    """Response from taking a screenshot."""

    session_id: str
    status: Literal["completed"]
    current_url: str | None = None
    artifact: dict[str, Any]
