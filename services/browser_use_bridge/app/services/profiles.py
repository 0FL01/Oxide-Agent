"""Profile management: creation, lifecycle, persistence, housekeeping."""

from __future__ import annotations

import asyncio
import json
import shutil
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4

from fastapi import HTTPException

from app.models.internal import ProfileRecord
from app.utils.time import utc_now, parse_timestamp
from app.utils.text import clean_optional


class ProfileManager:
    """Manages persistent browser profiles."""

    def __init__(
        self,
        profiles_dir: Path,
        max_profiles_per_scope: int = 3,
        profile_idle_ttl_secs: int = 604800,
    ) -> None:
        self._profiles_dir = profiles_dir
        self._max_profiles_per_scope = max(1, max_profiles_per_scope)
        self._profile_idle_ttl_secs = max(0, profile_idle_ttl_secs)
        self._profiles: dict[str, ProfileRecord] = {}
        self._registry_lock = asyncio.Lock()
        self._profiles_dir.mkdir(parents=True, exist_ok=True)

    async def get_profile(self, profile_id: str) -> ProfileRecord:
        """Get profile by ID, loading from disk if needed."""
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

    async def create_profile(
        self, profile_scope: str = "bridge_local"
    ) -> ProfileRecord:
        """Create a new persistent profile."""
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

    async def attach_profile(self, session_id: str, profile: ProfileRecord) -> None:
        """Attach profile to session."""
        async with profile.lock:
            if (
                profile.current_session_id is not None
                and profile.current_session_id != session_id
            ):
                raise HTTPException(
                    status_code=409,
                    detail=(
                        f"profile '{profile.profile_id}' is already attached to "
                        f"session '{profile.current_session_id}'"
                    ),
                )
            profile.current_session_id = session_id
            profile.status = "active"
            profile.last_used_at = utc_now()
            profile.updated_at = utc_now()
            await self._persist_profile(profile)

    async def detach_profile(self, session_id: str, profile_id: str) -> ProfileRecord:
        """Detach profile from session."""
        try:
            profile = await self.get_profile(profile_id)
        except HTTPException:
            # Profile already gone - return minimal record
            return ProfileRecord(
                profile_id=profile_id,
                profile_scope="bridge_local",
                status="idle",
            )

        async with profile.lock:
            if profile.current_session_id == session_id:
                profile.current_session_id = None
            profile.status = "idle"
            profile.updated_at = utc_now()
            await self._persist_profile(profile)

        return profile

    async def _enforce_profile_scope_quota(self, profile_scope: str) -> None:
        """Enforce max profiles per scope quota."""
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
        """Count non-deleted profiles for scope."""
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

    async def _housekeep_profiles(self) -> None:
        """Run profile housekeeping: reconcile orphans and prune expired."""
        await self._reconcile_orphaned_profiles()
        await self._prune_expired_profiles()

    async def _reconcile_orphaned_profiles(
        self, live_sessions: dict[str, dict[str, Any]] | None = None
    ) -> None:
        """Mark profiles as stale if their session is gone."""
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
                and live_sessions is not None
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
        """Delete expired idle profiles."""
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
        """Load profile metadata from disk."""
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
        """Write profile metadata to disk."""
        metadata_path.parent.mkdir(parents=True, exist_ok=True)
        metadata_path.write_text(
            json.dumps(payload, ensure_ascii=True, indent=2),
            encoding="utf-8",
        )

    async def _persist_profile(self, profile: ProfileRecord) -> None:
        """Persist profile to disk."""
        metadata_path = self._profile_metadata_path(profile.profile_id)
        metadata_path.parent.mkdir(parents=True, exist_ok=True)
        metadata_path.write_text(
            json.dumps(profile.snapshot(), ensure_ascii=True, indent=2),
            encoding="utf-8",
        )

    async def _sync_cached_profile_payload(self, payload: dict[str, Any]) -> None:
        """Sync loaded payload to in-memory cache."""
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

    def _session_snapshot_matches_profile(
        self, snapshot: dict[str, Any] | None, profile_id: str
    ) -> bool:
        """Check if session snapshot matches profile attachment."""
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
        """Check if profile payload is expired."""
        if payload.get("status") == "deleted":
            return True

        for key in ("last_used_at", "updated_at", "created_at"):
            parsed = parse_timestamp(payload.get(key))
            if parsed is not None:
                return parsed <= cutoff
        return False

    def _profile_root(self, profile_id: str) -> Path:
        return self._profiles_dir / profile_id

    def _profile_browser_dir(self, profile_id: str) -> Path:
        return self._profile_root(profile_id) / "browser"

    def _profile_metadata_path(self, profile_id: str) -> Path:
        return self._profile_root(profile_id) / "metadata.json"
