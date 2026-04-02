"""LLM configuration resolution and factory."""

from __future__ import annotations

import os
from typing import Any, Literal

from app.config import settings
from app.constants import (
    MINIMAX_DEFAULT_API_BASE,
    ZAI_DEFAULT_API_BASE,
    OPENAI_CHAT_COMPLETIONS_SUFFIX,
)
from app.models.requests import BrowserLlmConfig, RunTaskRequest
from app.models.internal import ResolvedBrowserLlmConfig
from app.utils.text import normalize_name, clean_optional

try:
    import browser_use as browser_use_module
except ImportError:  # pragma: no cover - exercised in runtime envs.
    browser_use_module = None

ChatAnthropic = getattr(browser_use_module, "ChatAnthropic", None)
ChatBrowserUse = getattr(browser_use_module, "ChatBrowserUse", None)
ChatGoogle = getattr(browser_use_module, "ChatGoogle", None)
ChatOpenAI = getattr(browser_use_module, "ChatOpenAI", None)


def resolve_api_key_ref(secret_ref: str) -> str:
    """Resolve API key from env:KEY reference."""
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
    """Normalize OpenAI-compatible API base URL."""
    cleaned = clean_optional(api_base)
    if cleaned is None:
        return None
    trimmed = cleaned.rstrip("/")
    if trimmed.endswith(OPENAI_CHAT_COMPLETIONS_SUFFIX):
        trimmed = trimmed[: -len(OPENAI_CHAT_COMPLETIONS_SUFFIX)]
    return trimmed or None


def infer_transport(provider: str, api_base: str | None, transport: str | None) -> str:
    """Infer transport type from provider and configuration."""
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
    """Resolve LLM configuration from request-provided config."""
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
    """Resolve LLM configuration from legacy environment variables."""
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
    """Resolve LLM configuration from request or legacy env."""
    if request.browser_llm_config is not None:
        return resolve_requested_llm_config(
            request.browser_llm_config, browser_llm_api_key
        )
    return resolve_legacy_llm_config()


def create_llm_from_config(config: ResolvedBrowserLlmConfig) -> Any:
    """Create LLM client instance from resolved config."""
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
    """Determine vision mode setting from config."""
    if config.supports_vision is False:
        return False
    return "auto"


def vision_mode_label(config: ResolvedBrowserLlmConfig) -> Literal["auto", "disabled"]:
    """Get vision mode label for response."""
    if config.supports_vision is False:
        return "disabled"
    return "auto"


def build_agent_task(request: RunTaskRequest) -> str:
    """Build agent task string from request."""
    task_parts = [request.task.strip()]
    if request.start_url:
        task_parts.append(f"Start from this URL: {request.start_url.strip()}")
    return "\n\n".join(task_parts)
