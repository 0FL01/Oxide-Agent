"""Request models for API endpoints."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, Field

ExecutionMode = Literal["autonomous", "navigation_only"]


class BrowserLlmConfig(BaseModel):
    """LLM configuration for browser tasks."""

    provider: str = Field(min_length=1)
    model: str | None = None
    api_base: str | None = None
    api_key_ref: str | None = None
    supports_vision: bool | None = None
    supports_tools: bool | None = None
    transport: str | None = None


class RunTaskRequest(BaseModel):
    """Request to run a browser task."""

    task: str = Field(min_length=1)
    start_url: str | None = None
    session_id: str | None = None
    timeout_secs: int | None = Field(default=None, ge=1)
    reuse_profile: bool = False
    profile_id: str | None = Field(default=None, min_length=1)
    profile_scope: str | None = Field(default=None, min_length=1)
    execution_mode: ExecutionMode | None = None
    browser_llm_config: BrowserLlmConfig | None = None


class ExtractContentRequest(BaseModel):
    """Request to extract content from browser."""

    format: Literal["text", "html"] = "text"
    max_chars: int | None = Field(default=12000, ge=1, le=100000)


class ScreenshotRequest(BaseModel):
    """Request to take a screenshot."""

    full_page: bool = False
