"""Session management: creation, task execution, lifecycle."""

from __future__ import annotations

import asyncio
import json
import logging
import mimetypes
from pathlib import Path
from typing import Any
from uuid import uuid4

from fastapi import HTTPException

from app.config import settings
from app.models.requests import (
    RunTaskRequest,
    ExtractContentRequest,
    ScreenshotRequest,
    ExecutionMode,
)
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
from app.utils.json_safe import classify_run_result, stringify_result, extract_artifacts
from app.utils.browser_utils import (
    infer_url,
    ensure_browser_runtime_ready,
    ensure_browser_session_alive,
    start_browser_runtime,
    is_browser_session_unavailable_error,
    probe_browser_session_state,
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

STEERING_TASK_PREFIX = "Browser Use execution rules for this run:"
NAVIGATION_ONLY_SYSTEM_MESSAGE = (
    "This run is navigation-only. Success means the browser is left on the target "
    "page or UI state for Oxide follow-up tools. Do not take screenshots, save PDFs, "
    "download files, or perform final content extraction in this run. Stop once the "
    "requested page or UI state is ready and return a short readiness summary. If the "
    "page still shows only loading placeholders, skeletons, or spinners, wait for real "
    "content or retry a single refresh/navigation. Do not loop on repeated identical "
    "wait actions; after one extra wait or refresh, stop and report the blocking "
    "loading state."
)

TRANSIENT_AGENT_OUTPUT_ERROR_PATTERNS = [
    "pydantic.json_invalid",
    "json_invalid",
    "invalid json",
    "trailing characters",
    "extra data",
    "empty model response",
    "empty response",
    "agent did not return any actions",
]


def resolve_execution_mode(
    task: str, requested_mode: ExecutionMode | None
) -> ExecutionMode:
    """Resolve execution mode from explicit request setting or legacy steering wrapper."""
    if requested_mode is not None:
        return requested_mode
    if task.lstrip().startswith(STEERING_TASK_PREFIX):
        return "navigation_only"
    return "autonomous"


def navigation_only_agent_kwargs(execution_mode: ExecutionMode) -> dict[str, Any]:
    """Return stricter Agent kwargs for runs that should stay navigation-only."""
    if execution_mode != "navigation_only":
        return {}

    return {
        "enable_planning": False,
        "use_judge": False,
        "max_actions_per_step": 1,
        "extend_system_message": NAVIGATION_ONLY_SYSTEM_MESSAGE,
    }


def browser_keep_alive_enabled(execution_mode: ExecutionMode) -> bool:
    """Navigation-only runs keep the upstream browser alive for follow-up tools."""
    return execution_mode == "navigation_only"


def detect_browser_keep_alive_effective(browser: Any) -> bool:
    """Detect whether keep-alive is effectively enabled on the live browser object."""
    profile = getattr(browser, "browser_profile", None)
    if profile is not None:
        return getattr(profile, "keep_alive", None) is True
    return getattr(browser, "keep_alive", None) is True


def is_transient_agent_output_error(error: Exception | str | None) -> bool:
    """Check if a browser-use run failed because the model returned unusable output."""
    if error is None:
        return False

    message = error if isinstance(error, str) else str(error)
    message = message.strip().lower()
    if not message:
        return False

    return any(pattern in message for pattern in TRANSIENT_AGENT_OUTPUT_ERROR_PATTERNS)


def _detect_content_type(path: Path) -> str:
    guessed, _ = mimetypes.guess_type(path.name)
    return guessed or "application/octet-stream"


def _iter_download_paths(browser: Any) -> list[Path]:
    downloaded_files = getattr(browser, "downloaded_files", None)
    if not isinstance(downloaded_files, list):
        return []

    paths: list[Path] = []
    for candidate in downloaded_files:
        if not isinstance(candidate, str):
            continue
        candidate_path = Path(candidate)
        if candidate_path.exists() and candidate_path.is_file():
            paths.append(candidate_path)
    return paths


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

    def _downloads_dir(self, session_id: str) -> Path:
        return self._artifacts_dir / session_id / "downloads"

    def _collect_download_artifacts(
        self,
        browser: Any,
        session_id: str,
        existing: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        artifacts = list(existing)
        existing_ids = {
            str(artifact.get("artifact_id"))
            for artifact in artifacts
            if isinstance(artifact, dict) and artifact.get("artifact_id")
        }

        for path in _iter_download_paths(browser):
            artifact_id = path.name
            if artifact_id in existing_ids:
                continue

            artifacts.append(
                {
                    "kind": "download",
                    "artifact_id": artifact_id,
                    "file_name": artifact_id,
                    "content_type": _detect_content_type(path),
                    "download_path": f"/sessions/{session_id}/artifacts/{artifact_id}",
                    "path": str(path),
                    "size_bytes": path.stat().st_size,
                    "created_at": utc_now(),
                }
            )
            existing_ids.add(artifact_id)

        return artifacts

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
            session.execution_mode = resolve_execution_mode(
                request.task, request.execution_mode
            )
            session.browser_keep_alive_requested = browser_keep_alive_enabled(
                session.execution_mode
            )
            self._reset_browser_reconnect_observability(session)
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

                    result, run_error = await self._run_agent_with_browser_ready_retry(
                        session=session,
                        request=request,
                        profile=profile,
                        llm_config=llm_config,
                        timeout_secs=timeout_secs,
                    )
                    session.summary = stringify_result(result)
                    session.artifacts = self._collect_download_artifacts(
                        session.browser,
                        session.session_id,
                        extract_artifacts(result),
                    )
                    session.current_url = await infer_url(session.browser, result)
                    await self._refresh_browser_runtime_observability(session)
                    if run_error is None:
                        session.status = "completed"
                    else:
                        session.status = "failed"
                        session.last_error = run_error
                        if is_browser_session_unavailable_error(run_error):
                            await self._refresh_browser_runtime_observability(
                                session, dead_reason=run_error
                            )
                except Exception as error:
                    session.status = "failed"
                    session.last_error = str(error)
                    if is_browser_session_unavailable_error(error):
                        await self._refresh_browser_runtime_observability(
                            session, dead_reason=str(error)
                        )
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
            execution_mode=session.execution_mode,
            profile_id=session.profile_id,
            profile_scope=session.profile_scope,
            profile_status=session.profile_status,
            profile_attached=session.profile_attached,
            profile_reused=profile_reused,
            browser_runtime_alive=session.browser_runtime_alive,
            browser_runtime_last_check_at=session.browser_runtime_last_check_at,
            browser_runtime_dead_reason=session.browser_runtime_dead_reason,
            browser_keep_alive_requested=session.browser_keep_alive_requested,
            browser_keep_alive_effective=session.browser_keep_alive_effective,
            browser_reconnect_attempted=session.browser_reconnect_attempted,
            browser_reconnect_succeeded=session.browser_reconnect_succeeded,
            browser_reconnect_error=session.browser_reconnect_error,
        )

    async def close_session(self, session_id: str) -> CloseSessionResponse:
        """Close a session and cleanup resources."""
        session = await self.get_session(session_id)
        await self._close_session_record(session)
        return CloseSessionResponse(
            session_id=session_id,
            closed=True,
            status="closed",
            execution_mode=session.execution_mode,
            profile_id=session.profile_id,
            profile_scope=session.profile_scope,
            profile_status=session.profile_status,
            profile_attached=session.profile_attached,
            browser_runtime_alive=session.browser_runtime_alive,
            browser_runtime_last_check_at=session.browser_runtime_last_check_at,
            browser_runtime_dead_reason=session.browser_runtime_dead_reason,
            browser_keep_alive_requested=session.browser_keep_alive_requested,
            browser_keep_alive_effective=session.browser_keep_alive_effective,
            browser_reconnect_attempted=session.browser_reconnect_attempted,
            browser_reconnect_succeeded=session.browser_reconnect_succeeded,
            browser_reconnect_error=session.browser_reconnect_error,
        )

    async def extract_content(
        self, session_id: str, request: ExtractContentRequest
    ) -> ExtractContentResponse:
        """Extract content from browser page."""
        await self.ensure_runtime_ready()
        session = await self.get_session(session_id)

        async with session.lock:
            try:
                self._reset_browser_reconnect_observability(session)
                browser = await self._ensure_follow_up_browser_session(session)
                content, truncated, total_chars = await extract_content(
                    browser, request.format, request.max_chars
                )
                current_url = await infer_url(browser, None)
                session.current_url = current_url
                await self._refresh_browser_runtime_observability(
                    session, browser=browser
                )
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
                self._reset_browser_reconnect_observability(session)
                browser = await self._ensure_follow_up_browser_session(session)
                artifact = await take_screenshot(
                    browser,
                    self._artifacts_dir,
                    session.session_id,
                    request.full_page,
                )
                session.artifacts.append(artifact)
                current_url = await infer_url(browser, None)
                session.current_url = current_url
                await self._refresh_browser_runtime_observability(
                    session, browser=browser
                )
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

    async def get_artifact_path(
        self, session_id: str, artifact_id: str
    ) -> tuple[Path, str]:
        """Resolve an artifact path and content type for download endpoints."""
        session = await self.get_session(session_id)
        normalized_artifact_id = Path(artifact_id).name
        if not normalized_artifact_id or normalized_artifact_id != artifact_id:
            raise HTTPException(status_code=404, detail="unknown artifact")

        for artifact in session.artifacts:
            if not isinstance(artifact, dict):
                continue
            if artifact.get("artifact_id") != normalized_artifact_id:
                continue

            path = Path(str(artifact.get("path") or ""))
            if path.exists() and path.is_file():
                content_type = str(artifact.get("content_type") or "").strip()
                if not content_type:
                    content_type = _detect_content_type(path)
                return path, content_type
            break

        raise HTTPException(status_code=404, detail="unknown artifact")

    async def shutdown(self) -> None:
        """Shutdown all sessions."""
        async with self._registry_lock:
            sessions = list(self._sessions.values())
        for session in sessions:
            await self._close_session_record(session)

    async def _close_session_record(self, session: SessionRecord) -> None:
        """Close a session record and cleanup."""
        async with session.lock:
            await close_browser(session.browser, kill=True)
            session.browser = None
            session.browser_keep_alive_effective = False
            session.status = "closed"
            self._reset_browser_reconnect_observability(session)
            self._set_browser_runtime_observability(
                session,
                alive=False,
                dead_reason="browser session was closed by bridge",
            )

            if session.profile_id is not None:
                live_sessions = await self._live_session_snapshots()
                profile = await self._profile_manager.detach_profile(
                    session.session_id,
                    session.profile_id,
                    live_sessions=live_sessions,
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
    ) -> tuple[Any, str | None]:
        """Run agent with retry logic for browser readiness."""
        max_attempts = self._browser_ready_retries + 1
        for attempt in range(1, max_attempts + 1):
            try:
                if session.browser is None:
                    downloads_dir = self._downloads_dir(session.session_id)
                    downloads_dir.mkdir(parents=True, exist_ok=True)
                    session.browser = create_browser(
                        profile,
                        keep_alive=browser_keep_alive_enabled(
                            session.execution_mode or "autonomous"
                        ),
                        downloads_path=str(downloads_dir),
                    )
                session.browser_keep_alive_effective = (
                    detect_browser_keep_alive_effective(session.browser)
                )

                await self._warmup_browser_before_run(session)

                task = build_agent_task(request)
                agent_kwargs = navigation_only_agent_kwargs(
                    session.execution_mode or "autonomous"
                )

                agent = Agent(
                    task=task,
                    llm=create_llm_from_config(llm_config),
                    browser=session.browser,
                    use_vision=use_vision_mode(llm_config),
                    **agent_kwargs,
                )
                result = await asyncio.wait_for(agent.run(), timeout=timeout_secs)
                success, run_error = classify_run_result(result)
                if success:
                    return result, None
                retryable_run_error = is_transient_browser_ready_error(
                    RuntimeError(run_error or "")
                ) or is_transient_agent_output_error(run_error)
                if attempt >= max_attempts or not retryable_run_error:
                    return result, run_error

                logger.warning(
                    "Retrying Browser Use after transient agent failure",
                    extra={
                        "session_id": session.session_id,
                        "attempt": attempt,
                        "max_attempts": max_attempts,
                        "error": run_error,
                    },
                )
                await self._reset_browser_for_retry(session)
                if self._browser_ready_retry_delay_ms > 0:
                    await asyncio.sleep(self._browser_ready_retry_delay_ms / 1000)
                continue

            except Exception as error:
                retryable_error = is_transient_browser_ready_error(
                    error
                ) or is_transient_agent_output_error(error)
                if attempt >= max_attempts or not retryable_error:
                    raise

                logger.warning(
                    "Retrying Browser Use after transient agent failure",
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
        await close_browser(session.browser, kill=True)
        session.browser = None
        session.browser_keep_alive_effective = False

    async def _warmup_browser_before_run(self, session: SessionRecord) -> None:
        """Wait briefly for a fresh browser runtime before the first agent step."""
        browser = session.browser
        if browser is None:
            return

        max_checks = 2
        for warmup_attempt in range(1, max_checks + 1):
            try:
                await ensure_browser_runtime_ready(browser)
                return
            except Exception as error:
                if warmup_attempt >= max_checks or not is_transient_browser_ready_error(
                    error
                ):
                    raise

                logger.info(
                    "Waiting for Browser Use runtime before agent start",
                    extra={
                        "session_id": session.session_id,
                        "warmup_attempt": warmup_attempt,
                        "max_checks": max_checks,
                        "error": str(error),
                    },
                )
                if self._browser_ready_retry_delay_ms > 0:
                    await asyncio.sleep(self._browser_ready_retry_delay_ms / 1000)

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
                profile = await self._profile_manager.get_profile(
                    session.profile_id,
                    live_sessions=await self._live_session_snapshots(),
                )
                return profile, True
            return None, False

        # No active browser - resolve from request
        if requested_profile_id is not None:
            profile = await self._profile_manager.get_profile(
                requested_profile_id,
                live_sessions=await self._live_session_snapshots(),
            )
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
                requested_profile_scope or "bridge_local",
                live_sessions=await self._live_session_snapshots(),
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
        await close_browser(session.browser, kill=True)
        session.browser = None
        session.browser_keep_alive_effective = False
        session.last_error = reason
        self._set_browser_runtime_observability(
            session,
            alive=False,
            dead_reason=reason,
        )
        session.updated_at = utc_now()
        await self._persist(session)

    async def _refresh_browser_runtime_observability(
        self,
        session: SessionRecord,
        browser: Any | None = None,
        dead_reason: str | None = None,
    ) -> None:
        """Refresh observable browser runtime state for this session."""
        browser = session.browser if browser is None else browser
        alive, probed_reason = await probe_browser_session_state(browser)
        self._set_browser_runtime_observability(
            session,
            alive=alive,
            dead_reason=dead_reason or probed_reason,
        )

    def _set_browser_runtime_observability(
        self,
        session: SessionRecord,
        *,
        alive: bool,
        dead_reason: str | None,
    ) -> None:
        session.browser_runtime_alive = alive
        session.browser_runtime_last_check_at = utc_now()
        session.browser_runtime_dead_reason = None if alive else dead_reason

    async def _ensure_follow_up_browser_session(self, session: SessionRecord) -> Any:
        """Ensure follow-up tools have a live browser session, with optional reconnect."""
        browser = self._require_active_browser(session)
        try:
            await ensure_browser_session_alive(browser)
            return browser
        except Exception as error:
            return await self._attempt_follow_up_browser_reconnect(
                session,
                browser,
                error,
            )

    async def _attempt_follow_up_browser_reconnect(
        self,
        session: SessionRecord,
        browser: Any,
        error: Exception,
    ) -> Any:
        """Try one reconnect attempt for navigation-only keep-alive sessions."""
        if not self._should_attempt_follow_up_browser_reconnect(
            session=session,
            browser=browser,
            error=error,
        ):
            raise error

        logger.info(
            "Attempting Browser Use reconnect before follow-up tool",
            extra={
                "session_id": session.session_id,
                "execution_mode": session.execution_mode,
                "error": str(error),
            },
        )

        try:
            await start_browser_runtime(browser)
            await ensure_browser_session_alive(browser)
        except Exception as reconnect_error:
            self._set_browser_reconnect_observability(
                session,
                attempted=True,
                succeeded=False,
                error=str(reconnect_error),
            )
            raise reconnect_error from error

        session.browser_keep_alive_effective = detect_browser_keep_alive_effective(
            browser
        )
        await self._refresh_browser_runtime_observability(session, browser=browser)
        self._set_browser_reconnect_observability(
            session,
            attempted=True,
            succeeded=True,
            error=None,
        )
        return browser

    def _should_attempt_follow_up_browser_reconnect(
        self,
        *,
        session: SessionRecord,
        browser: Any,
        error: Exception,
    ) -> bool:
        """Reconnect only for navigation-only keep-alive sessions with dead runtime."""
        if session.execution_mode != "navigation_only":
            return False

        keep_alive_effective = session.browser_keep_alive_effective is True
        if not keep_alive_effective:
            keep_alive_effective = detect_browser_keep_alive_effective(browser)
        if not keep_alive_effective:
            return False

        if isinstance(error, HTTPException):
            detail = error.detail
            if isinstance(detail, dict):
                detail_error = str(detail.get("error") or "")
                detail_message = str(detail.get("message") or "")
                return detail_error == "browser_session_not_alive" or (
                    is_browser_session_unavailable_error(detail_message)
                )
            return isinstance(detail, str) and is_browser_session_unavailable_error(
                detail
            )

        return is_browser_session_unavailable_error(error)

    def _reset_browser_reconnect_observability(self, session: SessionRecord) -> None:
        self._set_browser_reconnect_observability(
            session,
            attempted=None,
            succeeded=None,
            error=None,
        )

    def _set_browser_reconnect_observability(
        self,
        session: SessionRecord,
        *,
        attempted: bool | None,
        succeeded: bool | None,
        error: str | None,
    ) -> None:
        session.browser_reconnect_attempted = attempted
        session.browser_reconnect_succeeded = succeeded
        session.browser_reconnect_error = error

    async def _live_session_snapshots(self) -> dict[str, dict[str, Any]]:
        """Return current session snapshots keyed by session_id."""
        async with self._registry_lock:
            sessions = list(self._sessions.values())
        return {session.session_id: session.snapshot() for session in sessions}

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
