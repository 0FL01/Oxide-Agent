#!/usr/bin/env python3
"""Validate required secrets for deployment."""

import os
import sys


REQUIRED_SECRETS = [
    "DOCKER_IMAGE",
    "SHORT_SHA_VALUE",
    # Required for R2 storage
    "R2_ENDPOINT_URL",
    "R2_ACCESS_KEY_ID",
    "R2_SECRET_ACCESS_KEY",
    "R2_BUCKET_NAME",
    # Required for Telegram
    "TELEGRAM_TOKEN",
    "ALLOWED_USERS",
    # At least one LLM provider
    "GROQ_API_KEY",
    "OPENROUTER_API_KEY",
    "MISTRAL_API_KEY",
    "GEMINI_API_KEY",
]

AT_LEAST_ONE = [
    "GROQ_API_KEY",
    "OPENROUTER_API_KEY",
    "MISTRAL_API_KEY",
    "GEMINI_API_KEY",
]


def validate():
    """Validate all required environment variables."""
    missing = []
    llm_providers = []

    for key in REQUIRED_SECRETS:
        value = os.environ.get(key)
        if not value or value in ("dummy", "", "None"):
            if key in AT_LEAST_ONE:
                # Track separately for LLM providers
                llm_providers.append(key)
            else:
                missing.append(key)

    # Check if at least one LLM provider is configured
    if not any(os.environ.get(k) and os.environ.get(k) not in ("dummy", "", "None")
               for k in AT_LEAST_ONE):
        print("ERROR: At least one LLM provider API key must be set:")
        for provider in AT_LEAST_ONE:
            print(f"  - {provider}")
        sys.exit(1)

    if missing:
        print("ERROR: Missing required environment variables:")
        for key in missing:
            print(f"  - {key}")
        sys.exit(1)

    print("✅ All required secrets validated successfully")

    # Show configured LLM providers
    configured = [k for k in AT_LEAST_ONE
                  if os.environ.get(k) and os.environ.get(k) not in ("dummy", "", "None")]
    print(f"✅ LLM providers: {', '.join(configured)}")

    # Show deployment info
    print(f"📦 Docker image: {os.environ.get('DOCKER_IMAGE')}:{os.environ.get('SHORT_SHA_VALUE')}")
    print(f"🪣 R2 Bucket: {os.environ.get('R2_BUCKET_NAME')}")
    print(f"👥 Allowed users: {len(os.environ.get('ALLOWED_USERS', '').split(','))}")


if __name__ == "__main__":
    validate()
