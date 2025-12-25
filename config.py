import os
from groq import AsyncGroq
from dotenv import load_dotenv
from mistralai import Mistral
from typing import Optional, Set
import logging
from google import genai
from google.genai import types
import httpx
from pydantic import Field, computed_field
from pydantic_settings import BaseSettings, SettingsConfigDict

logger = logging.getLogger(__name__)

load_dotenv()

class Settings(BaseSettings):
    """Application settings with validation"""
    telegram_token: str
    allowed_users_str: str = Field(default='', validation_alias='ALLOWED_USERS')
    
    # API Keys
    groq_api_key: Optional[str] = None
    mistral_api_key: Optional[str] = None
    gemini_api_key: Optional[str] = None
    openrouter_api_key: Optional[str] = None
    
    # R2 Storage
    r2_access_key_id: Optional[str] = None
    r2_secret_access_key: Optional[str] = None
    r2_endpoint_url: Optional[str] = None
    r2_bucket_name: Optional[str] = None
    
    # OpenRouter configuration
    openrouter_site_url: str = ''
    openrouter_site_name: str = 'Another Chat TG Bot'
    
    # System message
    system_message: Optional[str] = None
    
    model_config = SettingsConfigDict(
        env_file='.env',
        case_sensitive=False,
    )
    
    @computed_field
    @property
    def allowed_users(self) -> Set[int]:
        """Parse comma-separated user IDs from string"""
        if not self.allowed_users_str or not self.allowed_users_str.strip():
            return set()
        try:
            return {int(x.strip()) for x in self.allowed_users_str.split(',') if x.strip()}
        except ValueError as e:
            logger.warning(f"Failed to parse allowed_users: {e}")
            return set()

# Initialize settings
settings = Settings()

# Legacy exports for backward compatibility
TELEGRAM_TOKEN = settings.telegram_token
GROQ_API_KEY = settings.groq_api_key
MISTRAL_API_KEY = settings.mistral_api_key
GEMINI_API_KEY = settings.gemini_api_key
OPENROUTER_API_KEY = settings.openrouter_api_key

# OpenRouter configuration
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1/chat/completions"
OPENROUTER_SITE_URL = settings.openrouter_site_url
OPENROUTER_SITE_NAME = settings.openrouter_site_name

# Client initialization is done below in the try/except block

MODELS = {
    "OR Gemini 3 Flash": {"id": "google/gemini-3-flash-preview", "max_tokens": 64000, "provider": "openrouter"},
    #"Gemini 2.5 Flash": {"id": "gemini-flash-latest", "max_tokens": 64000, "provider": "gemini"},
    "GPT-OSS-120b": {"id": "openai/gpt-oss-120b", "max_tokens": 64000, "provider": "groq"},
    "Mistral Large": {"id": "mistral-large-latest", "max_tokens": 128000, "provider": "mistral"},
    "Gemini 2.5 Flash Lite": {"id": "gemini-2.5-flash-lite", "max_tokens": 64000, "provider": "gemini"},
    #"Llama 3.3 70B 8K (groq)": {"id": "llama-3.3-70b-versatile", "max_tokens": 32000, "provider": "groq"}
}

DEFAULT_MODEL = "OR Gemini 3 Flash"

try:
    groq_client = AsyncGroq(api_key=GROQ_API_KEY) if GROQ_API_KEY else None
    mistral_client = Mistral(api_key=MISTRAL_API_KEY) if MISTRAL_API_KEY else None
    gemini_client = genai.Client(api_key=GEMINI_API_KEY) if GEMINI_API_KEY else None
    # OpenRouter использует httpx.AsyncClient для асинхронных запросов
    openrouter_client = httpx.AsyncClient(timeout=120.0) if OPENROUTER_API_KEY else None
except Exception as e:
    logger.error(f"Error initializing API clients: {str(e)}")
    groq_client = None
    mistral_client = None
    gemini_client = None
    openrouter_client = None

if not OPENROUTER_API_KEY:
    print("Warning: OPENROUTER_API_KEY is not set in the environment variables.")

