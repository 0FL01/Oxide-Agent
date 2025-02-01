import os
from groq import AsyncGroq
from dotenv import load_dotenv
from database import is_user_allowed, add_allowed_user, remove_allowed_user, UserRole
from utils import encode_image, process_file
from openai import OpenAI
from mistralai import Mistral
from together import Together
import base64
import pandas as pd
from typing import Union
import logging
import google.generativeai as genai


logger = logging.getLogger(__name__)

load_dotenv()

TELEGRAM_TOKEN = os.getenv('TELEGRAM_TOKEN')
GROQ_API_KEY = os.getenv('GROQ_API_KEY')
HF_API_KEY = os.getenv('HF_API_KEY')
OPENROUTER_API_KEY = os.getenv('OPENROUTER_API_KEY')
TOGETHER_API_KEY = os.getenv('TOGETHER_API_KEY')
MISTRAL_API_KEY = os.getenv('MISTRAL_API_KEY')
groq_client = AsyncGroq(api_key=GROQ_API_KEY)
GITHUB_TOKEN = os.getenv('GITHUB_TOKEN')
GEMINI_API_KEY = os.getenv('GEMINI_API_KEY')


AZURE_ENDPOINT = "https://models.inference.ai.azure.com"

if GITHUB_TOKEN:
    azure_client = OpenAI(
        base_url=AZURE_ENDPOINT,
        api_key=GITHUB_TOKEN,
    )
else:
    print("Warning: GITHUB_TOKEN is not set in the environment variables.")
    azure_client = None


if OPENROUTER_API_KEY:
    openrouter_client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=OPENROUTER_API_KEY,
    )
else:
    print("Warning: OPENROUTER_API_KEY is not set in the environment variables.")
    openrouter_client = None

if TOGETHER_API_KEY:
    together_client = Together(
        base_url="https://api.together.xyz/v1",
        api_key=TOGETHER_API_KEY,
    )
else:
    print("Warning: TOGETHER_API_KEY is not set in the environment variables.")
    together_client = None

if HF_API_KEY:
    huggingface_client = OpenAI(
        base_url="https://api-inference.huggingface.co/v1/",
        api_key=HF_API_KEY,
    )
else:
    print("Warning: HF_API_KEY is not set in the environment variables.")
    huggingface_client = None


if MISTRAL_API_KEY:
    mistral_client = Mistral(api_key=MISTRAL_API_KEY)
else:
    print("Warning: MISTRAL_API_KEY is not set in the environment variables.")
    mistral_client = None

if GEMINI_API_KEY:
    genai.configure(api_key=GEMINI_API_KEY)
    gemini_client = genai
else:
    print("Warning: GEMINI_API_KEY is not set in the environment variables.")
    gemini_client = None

chat_history = {}

MODELS = {
    #"Gemini 2.0 Flash Thinking Experimental": {"id": "gemini-2.0-flash-thinking-exp-01-21", "max_tokens": 128000, "provider": "gemini", "vision": True},
    "Gemini 2.0 Flash Experimental": {"id": "gemini-2.0-flash-exp", "max_tokens": 8192, "provider": "gemini", "vision": True},
    "DeepSeek-R1": {"id": "DeepSeek-R1", "max_tokens": 8192, "provider": "azure"},
    "DeepSeek-R1-Distill-Llama-70B": {"id": "DeepSeek-R1-Distill-Llama-70B", "max_tokens": 8192, "provider": "groq"},
    "Mistral Large 128K": {"id": "mistral-large-latest", "max_tokens": 128000, "provider": "mistral"},
    "GPT-4o 8K (Azure)": {"id": "gpt-4o", "max_tokens": 8192, "provider": "azure", "vision": True},
    "GPT-4o-mini 16K (Azure)": {"id": "gpt-4o-mini", "max_tokens": 16192, "provider": "azure", "vision": True},
    "Llama 3.3 70B 8K (groq)": {"id": "llama-3.3-70b-versatile", "max_tokens": 8000, "provider": "groq"},
    "FLUX.1-schnell": {"id": "black-forest-labs/FLUX.1-schnell-Free", "provider": "together", "type": "image"}
}

#DEFAULT_MODEL = "DeepSeek-R1-Distill-Llama-70B"
DEFAULT_MODEL = "Llama 3.3 70B 8K (groq)"

try:
    huggingface_client = OpenAI(
        base_url="https://api-inference.huggingface.co/v1/",
        api_key=HF_API_KEY,
    ) if HF_API_KEY else None
    azure_client = OpenAI(
        base_url=AZURE_ENDPOINT,
        api_key=GITHUB_TOKEN,
    ) if GITHUB_TOKEN else None
    together_client = Together(
        base_url="https://api.together.xyz/v1",
        api_key=TOGETHER_API_KEY,
    ) if TOGETHER_API_KEY else None
    groq_client = AsyncGroq(api_key=GROQ_API_KEY) if GROQ_API_KEY else None
    openrouter_client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=OPENROUTER_API_KEY,
    ) if OPENROUTER_API_KEY else None
    mistral_client = Mistral(api_key=MISTRAL_API_KEY) if MISTRAL_API_KEY else None
    gemini_client = genai if GEMINI_API_KEY else None
except Exception as e:
    logger.error(f"Error initializing API clients: {str(e)}")
    # Установите значения клиентов в None в случае ошибки
    huggingface_client = None
    azure_client = None
    together_client = None
    groq_client = None
    openrouter_client = None
    mistral_client = None
    gemini_client = None






