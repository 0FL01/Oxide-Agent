"""Configuration and settings for browser_use_bridge."""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path


def parse_int_env(name: str, default: int) -> int:
    """Parse integer from environment variable."""
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return default
    try:
        return int(raw)
    except ValueError:
        return default


@dataclass(frozen=True)
class Settings:
    """Application settings loaded from environment."""

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


# Global settings instance
settings = Settings()
os.environ.setdefault("BROWSER_USE_HOME", str(settings.data_dir))
