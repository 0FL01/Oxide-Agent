import os
from groq import AsyncGroq
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state, encode_image, process_file
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
    "Gemini 2.0 Flash Thinking Experimental": {"id": "gemini-2.0-flash-thinking-exp-01-21", "max_tokens": 128000, "provider": "gemini", "vision": True},
    "Gemini 2.0 Flash Experimental": {"id": "gemini-2.0-flash-exp", "max_tokens": 8192, "provider": "gemini", "vision": True},
    "DeepSeek-V3": {"id": "deepseek-ai/DeepSeek-V3", "max_tokens": 8192, "provider": "together"},
    "DeepSeek-R1": {"id": "deepseek-ai/DeepSeek-R1", "max_tokens": 8192, "provider": "together"},
    "DeepSeek-R1-Distill-Llama-70B": {"id": "DeepSeek-R1-Distill-Llama-70B", "max_tokens": 8192, "provider": "groq"},
    "Mistral Large 128K": {"id": "mistral-large-latest", "max_tokens": 128000, "provider": "mistral"},
    "GPT-4o 8K (Azure)": {"id": "gpt-4o", "max_tokens": 8192, "provider": "azure", "vision": True},
    "GPT-4o-mini 16K (Azure)": {"id": "gpt-4o-mini", "max_tokens": 16192, "provider": "azure", "vision": True},
    "Llama 3.3 70B 8K (groq)": {"id": "llama-3.3-70b-versatile", "max_tokens": 8000, "provider": "groq"},
    "FLUX.1-schnell": {"id": "black-forest-labs/FLUX.1-schnell-Free", "provider": "together", "type": "image"}
}

#DEFAULT_MODEL = "DeepSeek-R1-Distill-Llama-70B"
DEFAULT_MODEL = "Llama 3.3 70B 8K (groq)"

def generate_image(prompt):
    if not TOGETHER_API_KEY:
        raise ValueError("TOGETHER_API_KEY is not set in the environment variables.")

    together_client = Together(api_key=TOGETHER_API_KEY)
    response = together_client.images.generate(
        prompt=prompt,
        model="black-forest-labs/FLUX.1-schnell-Free",
        width=1024,
        height=768,
        steps=1,
        n=1,
        response_format="b64_json"
    )
    return response.data[0].b64_json

ADMIN_ID = int(os.getenv('ADMIN_ID'))

def encode_image(image_path):
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode('utf-8')

def process_file(file_path: str, max_size: int = 1 * 1024 * 1024) -> str:
    if os.path.getsize(file_path) > max_size:
        raise ValueError(f"Файл слишком большой. Максимальный размер: {max_size/1024/1024}MB")

    file_extension = os.path.splitext(file_path)[1].lower()
    content = ""

    try:
        # Text-based files
        if file_extension in ['.txt', '.log', '.md']:
            with open(file_path, 'r', encoding='utf-8') as file:
                content = file.read()




