"""Text and string utilities."""


def normalize_name(value: str | None) -> str:
    """Normalize string: lowercase, strip, replace dashes with underscores."""
    if value is None:
        return ""
    return value.strip().lower().replace("-", "_")


def clean_optional(value: str | None) -> str | None:
    """Clean optional string: strip whitespace, return None if empty."""
    if value is None:
        return None
    cleaned = value.strip()
    return cleaned or None


def maybe_truncate(content: str, max_chars: int | None) -> tuple[str, bool, int]:
    """Truncate content if it exceeds max_chars.

    Returns: (content, was_truncated, total_chars)
    """
    total_chars = len(content)
    if max_chars is None or total_chars <= max_chars:
        return content, False, total_chars
    return content[:max_chars], True, total_chars
