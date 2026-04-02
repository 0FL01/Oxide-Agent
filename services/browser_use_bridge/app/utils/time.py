"""Time-related utilities."""

from datetime import datetime, timezone


def utc_now() -> str:
    """Return current UTC time as ISO format string."""
    return datetime.now(timezone.utc).isoformat()


def parse_timestamp(value: str | None) -> datetime | None:
    """Parse ISO timestamp string to datetime (UTC)."""
    if value is None:
        return None

    cleaned = value.strip()
    if not cleaned:
        return None

    try:
        parsed = datetime.fromisoformat(cleaned.replace("Z", "+00:00"))
    except ValueError:
        return None

    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)
