"""FastAPI application entry point for browser_use_bridge."""

from __future__ import annotations

from contextlib import asynccontextmanager

from fastapi import FastAPI, Header
from fastapi.responses import JSONResponse

try:
    import browser_use as browser_use_module
except ImportError:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None
    BROWSER_USE_IMPORT_ERROR = "browser_use import failed"
else:
    BROWSER_USE_IMPORT_ERROR = None

from app.config import settings
from app.constants import OXIDE_BROWSER_LLM_API_KEY_HEADER
from app.models.requests import (
    RunTaskRequest,
    ExtractContentRequest,
    ScreenshotRequest,
)
from app.models.responses import (
    RunTaskResponse,
    SessionResponse,
    CloseSessionResponse,
    ExtractContentResponse,
    ScreenshotResponse,
)
from app.services.session_manager import SessionManager
from app.utils.text import clean_optional


manager = SessionManager(
    data_dir=settings.data_dir,
    max_concurrent_sessions=settings.max_concurrent_sessions,
    max_profiles_per_scope=settings.max_profiles_per_scope,
    profile_idle_ttl_secs=settings.profile_idle_ttl_secs,
    browser_ready_retries=settings.browser_ready_retries,
    browser_ready_retry_delay_ms=settings.browser_ready_retry_delay_ms,
)


@asynccontextmanager
async def app_lifespan(_: FastAPI):
    """Application lifespan context manager."""
    try:
        yield
    finally:
        await manager.shutdown()


app = FastAPI(
    title="browser_use_bridge",
    version="0.1.0",
    lifespan=app_lifespan,
)


@app.get("/health")
async def health() -> JSONResponse:
    """Health check endpoint."""
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
        "execution_mode_split_supported": True,
        "browser_runtime_observability_supported": True,
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
    """Run a browser task."""
    return await manager.run_task(request, browser_llm_api_key)


@app.get("/sessions/{session_id}", response_model=SessionResponse)
async def get_session(session_id: str) -> SessionResponse:
    """Get session status."""
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
        execution_mode=session.execution_mode,
        profile_id=session.profile_id,
        profile_scope=session.profile_scope,
        profile_status=session.profile_status,
        profile_attached=session.profile_attached,
        browser_runtime_alive=session.browser_runtime_alive,
        browser_runtime_last_check_at=session.browser_runtime_last_check_at,
        browser_runtime_dead_reason=session.browser_runtime_dead_reason,
    )


@app.delete("/sessions/{session_id}", response_model=CloseSessionResponse)
async def delete_session(session_id: str) -> CloseSessionResponse:
    """Close a session."""
    return await manager.close_session(session_id)


@app.post(
    "/sessions/{session_id}/extract_content", response_model=ExtractContentResponse
)
async def extract_content_endpoint(
    session_id: str, request: ExtractContentRequest
) -> ExtractContentResponse:
    """Extract content from browser page."""
    return await manager.extract_content(session_id, request)


@app.post("/sessions/{session_id}/screenshot", response_model=ScreenshotResponse)
async def screenshot_endpoint(
    session_id: str, request: ScreenshotRequest
) -> ScreenshotResponse:
    """Take a screenshot."""
    return await manager.screenshot(session_id, request)
