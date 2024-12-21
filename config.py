import os
from groq import AsyncGroq
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from openai import OpenAI
from mistralai import Mistral
from together import Together
import base64
import json
import yaml
import xml.etree.ElementTree as ET
import docx
import openpyxl
import xlrd
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
HYPERBOLIC_API_KEY = os.getenv('HYPERBOLIC_API_KEY')
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


if HYPERBOLIC_API_KEY:
    hyperbolic_client = OpenAI(
        base_url="https://api.hyperbolic.xyz/v1",
        api_key=HYPERBOLIC_API_KEY,
    )
else:
    print("Warning: HYPERBOLIC_API_KEY is not set in the environment variables.")
    hyperbolic_client = None

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
    "Gemini Exp 1206": {"id": "gemini-exp-1206", "max_tokens": 256000, "provider": "gemini"},
    "Gemini 2.0 Flash Thinking Experimental": {"id": "gemini-2.0-flash-thinking-exp-1219", "max_tokens": 8192, "provider": "gemini"},
    "Gemini 2.0 Flash Experimental": {"id": "gemini-2.0-flash-exp", "max_tokens": 8192, "provider": "gemini"},
    "Mistral Large 128K": {"id": "mistral-large-latest", "max_tokens": 128000, "provider": "mistral"},
    "GPT-4o 8K (Azure)": {"id": "gpt-4o", "max_tokens": 8192, "provider": "azure", "vision": True},
    "GPT-4o-mini 16K (Azure)": {"id": "gpt-4o-mini", "max_tokens": 16192, "provider": "azure", "vision": True},
    "Llama 3.3 70B 8K (groq)": {"id": "llama-3.3-70b-versatile", "max_tokens": 8000, "provider": "groq"},
    "FLUX.1-schnell": {"id": "black-forest-labs/FLUX.1-schnell-Free", "provider": "together", "type": "image"}
}

DEFAULT_MODEL = "Gemini 2.0 Flash Experimental"
#DEFAULT_MODEL = "Llama 3.3 70B 8K (groq)"

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

        # XML files
        elif file_extension == '.xml':
            try:
                tree = ET.parse(file_path)
                root = tree.getroot()

                def process_element(element, level=0):
                    result = []
                    indent = "  " * level
                    attrib_str = ', '.join([f"{k}='{v}'" for k, v in element.attrib.items()])
                    tag_info = f"{element.tag}"
                    if attrib_str:
                        tag_info += f" ({attrib_str})"
                    result.append(f"{indent}{tag_info}")

                    if element.text and element.text.strip():
                        result.append(f"{indent}  {element.text.strip()}")

                    for child in element:
                        result.extend(process_element(child, level + 1))

                    return result

                content = "\n".join(process_element(root))
            except ET.ParseError as e:
                raise ValueError(f"Некорректный XML файл: {str(e)}")

        # Word documents
        elif file_extension in ['.docx', '.doc']:
            try:
                doc = docx.Document(file_path)
                paragraphs = []

                for paragraph in doc.paragraphs:
                    if paragraph.text.strip():
                        style = paragraph.style.name if paragraph.style else "Normal"
                        paragraphs.append(f"[{style}] {paragraph.text}")

                content = "\n\n".join(paragraphs)
            except Exception as e:
                raise ValueError(f"Ошибка при обработке документа Word: {str(e)}")

        # Excel files
        elif file_extension in ['.xlsx', '.xls']:
            try:
                if file_extension == '.xlsx':
                    df = pd.read_excel(file_path, engine='openpyxl')
                else:
                    df = pd.read_excel(file_path, engine='xlrd')

                content = (
                    f"Columns: {', '.join(df.columns)}\n"
                    f"Rows: {len(df)}\n\n"
                    f"{df.to_string(index=True, max_rows=1000)}"
                )
            except Exception as e:
                raise ValueError(f"Ошибка при обработке Excel файла: {str(e)}")

        # CSV files
        elif file_extension == '.csv':
            try:
                df = pd.read_csv(file_path)
                content = (
                    f"Columns: {', '.join(df.columns)}\n"
                    f"Rows: {len(df)}\n\n"
                    f"{df.to_string(index=True, max_rows=1000)}"
                )
            except Exception as e:
                raise ValueError(f"Ошибка при обработке CSV файла: {str(e)}")

        else:
            content = f"Unsupported file type: {file_extension}"

        # Add file metadata
        file_size = os.path.getsize(file_path) / 1024  # Size in KB
        file_name = os.path.basename(file_path)
        metadata = (
            f"File Information:\n"
            f"Name: {file_name}\n"
            f"Type: {file_extension}\n"
            f"Size: {file_size:.2f} KB\n"
            f"---\n\n"
        )

        return metadata + content

    except Exception as e:
        error_msg = f"Error processing file {file_path}: {str(e)}"
        logger.error(error_msg)
        raise ValueError(error_msg)




