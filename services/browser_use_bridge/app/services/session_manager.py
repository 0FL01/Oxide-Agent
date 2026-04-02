"""Session management: creation, task execution, lifecycle."""

from __future__ import annotations

import asyncio
import json
import logging
from pathlib import Path
from typing import Any
from uuid import uuid4

from fastapi import HTTPException

from app.config import settings
from app.models.requests import RunTaskRequest, ExtractContentRequest, ScreenshotRequest
from app.models.responses import (
    RunTaskResponse,
    CloseSessionResponse,
    ExtractContentResponse,
    ScreenshotResponse,
)
from app.models.internal import SessionRecord, ProfileRecord
from app.services.profiles import ProfileManager
from app.services.browser_ops import (
    create_browser,
    close_browser,
    extract_content,
    take_screenshot,
)
from app.services.llm_resolver import (
    resolve_llm_config,
    create_llm_from_config,
    use_vision_mode,
    vision_mode_label,
    build_agent_task,
)
from app.utils.json_safe import stringify_result, extract_artifacts
from app.utils.browser_utils import (
    infer_url,
    ensure_browser_session_alive,
    is_browser_session_unavailable_error,
    is_transient_browser_ready_error,
)
from app.utils.time import utc_now
from app.utils.text import clean_optional

try:
    import browser_use as browser_use_module
except ImportError:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None
    BROWSER_USE_IMPORT_ERROR = "browser_use import failed"
else:
    BROWSER_USE_IMPORT_ERROR = None

Agent = getattr(browser_use_module, "Agent", None)

logger = logging.getLogger(__name__)


class SessionManager:
    """Manages browser sessions and task execution."""

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
        self._sessions: dict[str, SessionRecord] = {}
        self._registry_lock = asyncio.Lock()
        self._semaphore = asyncio.Semaphore(max(1, max_concurrent_sessions))
        self._browser_ready_retries = max(0, browser_ready_retries)
        self._browser_ready_retry_delay_ms = max(0, browser_ready_retry_delay_ms)

        self._profile_manager = ProfileManager(
            profiles_dir=data_dir / "profiles",
            max_profiles_per_scope=max_profiles_per_scope,
            profile_idle_ttl_secs=profile_idle_ttl_secs,
        )

        self._sessions_dir.mkdir(parents=True, exist_ok=True)
        self._artifacts_dir.mkdir(parents=True, exist_ok=True)

    @property
    def profile_manager(self) -> ProfileManager:
        return self._profile_manager

    async def ensure_runtime_ready(self) -> None:
        """Check if browser_use runtime is available."""
        if Agent is None or browser_use_module is None:
            raise HTTPException(
                status_code=503,
                detail={
                    "error": "browser_use_unavailable",
                    "message": BROWSER_USE_IMPORT_ERROR or "browser_use import failed",
                },
            )

    async def get_session(self, session_id: str) -> SessionRecord:
        """Get session by ID."""
        async with self._registry_lock:
            session = self._sessions.get(session_id)
        if session is None:
            raise HTTPException(
                status_code=404, detail=f"unknown session '{session_id}'"
            )
        return session

    async def create_session(self) -> SessionRecord:
        """Create a new session."""
        session = SessionRecord(session_id=f"browser-use-{uuid4().hex}")
        async with self._registry_lock:
            self._sessions[session.session_id] = session
        await self._persist(session)
        return session

    async def get_or_create_session(self, session_id: str | None) -> SessionRecord:
        """Get existing session or create new one."""
        if session_id is None:
            return await self.create_session()
        return await self.get_session(session_id)

    async def run_task(
        self, request: RunTaskRequest, browser_llm_api_key: str | None
    ) -> RunTaskResponse:
        """Run a browser task in a session."""
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
                        await self._profile_manager.attach_profile(
                            session.session_id, profile
                        )
                        session.profile_id = profile.profile_id
                        session.profile_scope = profile.profile_scope
                        session.profile_status = profile.status
                        session.profile_attached = True
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
        """Close a session and cleanup resources."""
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
        """Extract content from browser page."""
        await self.ensure_runtime_ready()
        session = await self.get_session(session_id)

        async with session.lock:
            try:
                browser = self._require_active_browser(session)
                await ensure_browser_session_alive(browser)
                content, truncated, total_chars = await extract_content(
                    browser, request.format, request.max_chars
                )
                current_url = await infer_url(browser, None)
                session.current_url = current_url
                session.updated_at = utc_now()
                await self._persist(session)
            except Exception as error:
                await self._raise_follow_up_browser_error(session, error)

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
        """Take a screenshot of the current page."""
        await self.ensure_runtime_ready()
        session = await self.get_session(session_id)

        async with session.lock:
            try:
                browser = self._require_active_browser(session)
                await ensure_browser_session_alive(browser)
                artifact = await take_screenshot(
                    browser,
                    self._artifacts_dir,
                    session.session_id,
                    request.full_page,
                )
                session.artifacts.append(artifact)
                current_url = await infer_url(browser, None)
                session.current_url = current_url
                session.updated_at = utc_now()
                await self._persist(session)
            except Exception as error:
                await self._raise_follow_up_browser_error(session, error)

        return ScreenshotResponse(
            session_id=session.session_id,
            status="completed",
            current_url=current_url,
            artifact=artifact,
        )

    async def shutdown(self) -> None:
        """Shutdown all sessions."""
        async with self._registry_lock:
            sessions = list(self._sessions.values())
        for session in sessions:
            await self._close_session_record(session)

    async def _close_session_record(self, session: SessionRecord) -> None:
        """Close a session record and cleanup."""
        async with session.lock:
            await close_browser(session.browser)
            session.browser = None
            session.status = "closed"

            if session.profile_id is not None:
                profile = await self._profile_manager.detach_profile(
                    session.session_id, session.profile_id
                )
                session.profile_scope = profile.profile_scope
                session.profile_status = profile.status
                session.profile_attached = False

            session.updated_at = utc_now()
            await self._persist(session)

    async def _run_agent_with_browser_ready_retry(
        self,
        session: SessionRecord,
        request: RunTaskRequest,
        profile: ProfileRecord | None,
        llm_config: Any,
        timeout_secs: int,
    ) -> Any:
        """Run agent with retry logic for browser readiness."""
        max_attempts = self._browser_ready_retries + 1
        for attempt in range(1, max_attempts + 1):
            try:
                if session.browser is None:
                    session.browser = create_browser(profile)

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
        """Reset browser for retry attempt."""
        await close_browser(session.browser)
        session.browser = None

    async def _resolve_profile_for_run(
        self, session: SessionRecord, request: RunTaskRequest
    ) -> tuple[ProfileRecord | None, bool]:
        """Resolve profile for task execution."""
        requested_profile_id = clean_optional(request.profile_id)
        requested_profile_scope = clean_optional(request.profile_scope)

        # Session already has active browser
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
                profile = await self._profile_manager.get_profile(session.profile_id)
                return profile, True
            return None, False

        # No active browser - resolve from request
        if requested_profile_id is not None:
            profile = await self._profile_manager.get_profile(requested_profile_id)
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
            profile = await self._profile_manager.create_profile(
                requested_profile_scope or "bridge_local"
            )
            return profile, False

        return None, False

    def _require_active_browser(self, session: SessionRecord) -> Any:
        """Require session to have active browser."""
        if session.browser is None:
            raise HTTPException(
                status_code=409,
                detail=(
                    f"session '{session.session_id}' has no active browser; run "
                    "browser_use_run_task first"
                ),
            )
        return session.browser

    async def _raise_follow_up_browser_error(
        self, session: SessionRecord, error: Exception
    ) -> None:
        if isinstance(error, HTTPException):
            detail = error.detail
            if (
                isinstance(detail, dict)
                and detail.get("error") == "browser_session_not_alive"
            ):
                await self._mark_browser_session_unavailable(
                    session,
                    str(detail.get("message") or "browser session is not alive"),
                )
            elif isinstance(detail, str) and is_browser_session_unavailable_error(
                detail
            ):
                await self._mark_browser_session_unavailable(session, detail)
                raise self._browser_session_not_alive_error(session) from error
            raise error

        if is_browser_session_unavailable_error(error):
            await self._mark_browser_session_unavailable(session, str(error))
            raise self._browser_session_not_alive_error(session) from error

        raise error

    async def _mark_browser_session_unavailable(
        self, session: SessionRecord, reason: str
    ) -> None:
        await close_browser(session.browser)
        session.browser = None
        session.last_error = reason
        session.updated_at = utc_now()
        await self._persist(session)

    def _browser_session_not_alive_error(self, session: SessionRecord) -> HTTPException:
        return HTTPException(
            status_code=409,
            detail={
                "error": "browser_session_not_alive",
                "message": (
                    f"session '{session.session_id}' no longer has a live browser "
                    "runtime; run browser_use_run_task again before calling "
                    "browser_use_extract_content or browser_use_screenshot"
                ),
            },
        )

    async def _persist(self, session: SessionRecord) -> None:
        """Persist session to disk."""
        session_file = self._sessions_dir / f"{session.session_id}.json"
        session_file.write_text(
            json.dumps(session.snapshot(), ensure_ascii=True, indent=2),
            encoding="utf-8",
        )
