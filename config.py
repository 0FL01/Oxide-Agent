import os
from groq import AsyncGroq
from octoai.client import OctoAI
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from langchain.tools import DuckDuckGoSearchRun
from openai import OpenAI

load_dotenv()

TELEGRAM_TOKEN = os.getenv('TELEGRAM_TOKEN')
GROQ_API_KEY = os.getenv('GROQ_API_KEY')
OCTOAI_API_KEY = os.getenv('OCTOAI_API_KEY')
OPENROUTER_API_KEY = os.getenv('OPENROUTER_API_KEY')

groq_client = AsyncGroq(api_key=GROQ_API_KEY)
octoai_client = OctoAI(api_key=OCTOAI_API_KEY)

if OPENROUTER_API_KEY:
    openrouter_client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=OPENROUTER_API_KEY,
    )
else:
    print("Warning: OPENROUTER_API_KEY is not set in the environment variables.")
    openrouter_client = None

chat_history = {}
user_settings = {}

MODELS = {
    "Gemma 2 9B-8192": {"id": "gemma2-9b-it", "max_tokens": 8192, "provider": "groq"},
    "Gemini Flash 8B-1M": {"id": "google/gemini-flash-8b-1.5-exp", "max_tokens": 1000000, "provider": "openrouter"},
    "Reflection 70B-128K": {"id": "mattshumer/reflection-70b:free", "max_tokens": 128000, "provider": "openrouter"},
    "Llama 3.1 70B-8192": {"id": "llama-3.1-70b-versatile", "max_tokens": 8000, "provider": "groq"},
    "Llama 3.1 405B-128K": {"id": "meta-llama-3.1-405b-instruct", "max_tokens": 128000, "provider": "octoai"}
}

ADMIN_ID = int(os.getenv('ADMIN_ID'))

search_tool = DuckDuckGoSearchRun()
